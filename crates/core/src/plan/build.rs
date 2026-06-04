use super::{
    FilterColumnScope, FilterExpr, FilterLiteral, FragmentPlan, NestedRelation, OrderByPlan,
    PlanDiagnostic, PlanDiagnosticKind, PlannedFile, Projection, QueryPlan, SelectionClauses,
    SelectionPlan, SelectionPlanItem, SortDirectionPlan, SqlParameter, SqlValue, SqlVariantCase,
};
use crate::{
    VariableRole,
    catalog::{Catalog, FieldCheckResult, TableId, TableResolution},
    definition::{
        DefinitionResolver, FragmentMap, FragmentRecord, QueryRecord, extract_definitions,
    },
    syntax::{Definition, Selection, SelectionKind, SourceFile},
    variable_path::{
        InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope, variable_path,
    },
};

pub fn plan_file(source_file: &SourceFile) -> PlannedFile {
    plan_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn plan_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> PlannedFile {
    let extracted = extract_definitions(source_file);
    let resolver = FragmentMap::from_file(&extracted);
    let mut queries = Vec::new();
    let mut diagnostics = Vec::new();
    for definition in source_file.definitions() {
        let Definition::Query(query) = definition else {
            continue;
        };
        for selection in &query.selections {
            match catalog.resolve_table_ref(&selection.name.text) {
                TableResolution::Found(table) => {
                    let selection_path = vec![response_key(selection)];
                    let variable_scope = VariablePathScope::operation();
                    let clauses = plan_clauses(
                        catalog,
                        table.id,
                        table.id,
                        &selection_path,
                        &variable_scope,
                        selection,
                    );
                    if let Some(selections) = plan_selection_set(
                        catalog,
                        &resolver,
                        table.id,
                        table.id,
                        &clauses,
                        SelectionPath::body(selection_path),
                        &variable_scope,
                        &selection.selections,
                        &mut diagnostics,
                    ) {
                        queries.push(QueryPlan {
                            root: table.id,
                            output_name: selection
                                .alias
                                .as_ref()
                                .map_or_else(|| table.name.clone(), |alias| alias.text.clone()),
                            clauses,
                            selections,
                        });
                    }
                }
                TableResolution::NotFound { reference } => diagnostics.push(PlanDiagnostic {
                    range: selection.name.range,
                    kind: PlanDiagnosticKind::TableNotFound { table: reference },
                }),
                TableResolution::Ambiguous {
                    reference,
                    candidates,
                } => diagnostics.push(PlanDiagnostic {
                    range: selection.name.range,
                    kind: PlanDiagnosticKind::AmbiguousTable {
                        table: reference,
                        candidates,
                    },
                }),
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    PlannedFile {
        queries,
        diagnostics,
    }
}

pub fn plan_query_definition(
    query: &QueryRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> PlannedFile {
    let mut queries = Vec::new();
    let mut diagnostics = Vec::new();
    for selection in &query.selections {
        if selection.kind == SelectionKind::FragmentSpread {
            diagnostics.push(PlanDiagnostic {
                range: selection.name.range,
                kind: PlanDiagnosticKind::UnknownFragment {
                    fragment: selection.name.text.clone(),
                },
            });
            continue;
        }
        match catalog.resolve_table_ref(&selection.name.text) {
            TableResolution::Found(table) => {
                let selection_path = vec![response_key(selection)];
                let variable_scope = VariablePathScope::operation();
                let clauses = plan_clauses(
                    catalog,
                    table.id,
                    table.id,
                    &selection_path,
                    &variable_scope,
                    selection,
                );
                if let Some(selections) = plan_selection_set(
                    catalog,
                    resolver,
                    table.id,
                    table.id,
                    &clauses,
                    SelectionPath::body(selection_path),
                    &variable_scope,
                    &selection.selections,
                    &mut diagnostics,
                ) {
                    queries.push(QueryPlan {
                        root: table.id,
                        output_name: selection
                            .alias
                            .as_ref()
                            .map_or_else(|| table.name.clone(), |alias| alias.text.clone()),
                        clauses,
                        selections,
                    });
                }
            }
            TableResolution::NotFound { reference } => diagnostics.push(PlanDiagnostic {
                range: selection.name.range,
                kind: PlanDiagnosticKind::TableNotFound { table: reference },
            }),
            TableResolution::Ambiguous {
                reference,
                candidates,
            } => diagnostics.push(PlanDiagnostic {
                range: selection.name.range,
                kind: PlanDiagnosticKind::AmbiguousTable {
                    table: reference,
                    candidates,
                },
            }),
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    PlannedFile {
        queries,
        diagnostics,
    }
}

pub fn plan_fragment_definition(
    fragment: &FragmentRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> Option<FragmentPlan> {
    let mut diagnostics = Vec::new();
    let table = match fragment.on.as_deref() {
        Some(on) => match catalog.resolve_table_ref(on) {
            TableResolution::Found(table) => table.id,
            TableResolution::NotFound { reference } => {
                diagnostics.push(PlanDiagnostic {
                    range: fragment.on_range.unwrap_or(fragment.range),
                    kind: PlanDiagnosticKind::TableNotFound { table: reference },
                });
                return None;
            }
            TableResolution::Ambiguous {
                reference,
                candidates,
            } => {
                diagnostics.push(PlanDiagnostic {
                    range: fragment.on_range.unwrap_or(fragment.range),
                    kind: PlanDiagnosticKind::AmbiguousTable {
                        table: reference,
                        candidates,
                    },
                });
                return None;
            }
        },
        None => return None,
    };
    plan_selection_set(
        catalog,
        resolver,
        table,
        table,
        &SelectionClauses::default(),
        SelectionPath::fragment_root(),
        &VariablePathScope::fragment(),
        &fragment.selections,
        &mut diagnostics,
    )
    .map(|selections| FragmentPlan { table, selections })
}

fn plan_selection_set(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    root_table: TableId,
    table: TableId,
    clauses: &SelectionClauses,
    selection_path: SelectionPath,
    variable_scope: &VariablePathScope,
    selections: &[Selection],
    diagnostics: &mut Vec<PlanDiagnostic>,
) -> Option<SelectionPlan> {
    let mut items = Vec::new();
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            if let Some(fragment) = resolver.fragment(&selection.name.text)
                && let Some(fragment_plan) = plan_selection_set(
                    catalog,
                    resolver,
                    root_table,
                    table,
                    &SelectionClauses::default(),
                    SelectionPath::fragment_root(),
                    &variable_scope.for_fragment_spread(&selection_path, &fragment.key.name),
                    &fragment.selections,
                    diagnostics,
                )
            {
                items.extend(fragment_plan.items);
            }
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) => {
                if selection.selections.is_empty() {
                    items.push(SelectionPlanItem::Projection(Projection {
                        column: column.id,
                        output_name: selection
                            .alias
                            .as_ref()
                            .map_or_else(|| column.name.clone(), |alias| alias.text.clone()),
                    }));
                }
            }
            FieldCheckResult::Relation(relation) => {
                let child_path = relation_child_path(&selection_path, selection, relation.name);
                if let Some(nested) = plan_selection_set(
                    catalog,
                    resolver,
                    root_table,
                    relation.table.id,
                    &plan_clauses(
                        catalog,
                        root_table,
                        relation.table.id,
                        &child_path,
                        variable_scope,
                        selection,
                    ),
                    SelectionPath::body(child_path),
                    variable_scope,
                    &selection.selections,
                    diagnostics,
                ) {
                    items.push(SelectionPlanItem::Relation(NestedRelation {
                        relation_name: selection.name.text.clone(),
                        output_name: selection
                            .alias
                            .as_ref()
                            .map_or_else(|| relation.name.to_string(), |alias| alias.text.clone()),
                        table: relation.table.id,
                        foreign_key: relation.foreign_key.id,
                        selections: Box::new(nested),
                    }));
                }
            }
            FieldCheckResult::NotFound => {}
            FieldCheckResult::AmbiguousRelation {
                reference,
                candidates,
            } => diagnostics.push(PlanDiagnostic {
                range: selection.name.range,
                kind: PlanDiagnosticKind::AmbiguousRelation {
                    relation: reference,
                    candidates,
                },
            }),
        }
    }
    Some(SelectionPlan {
        table,
        clauses: clauses.clone(),
        items,
    })
}

fn relation_child_path(
    path: &SelectionPath,
    selection: &Selection,
    relation_name: &str,
) -> Vec<String> {
    path.relation_child_path(
        selection
            .alias
            .as_ref()
            .map_or_else(|| relation_name.to_string(), |alias| alias.text.clone()),
    )
}

fn plan_clauses(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    selection: &Selection,
) -> SelectionClauses {
    let mut clauses = SelectionClauses::default();
    for clause in &selection.clauses {
        match clause {
            crate::Clause::Where(where_clause) => {
                clauses.filter = plan_filter_expr(
                    catalog,
                    root_table,
                    table,
                    None,
                    selection_path,
                    variable_scope,
                    &where_clause.predicate,
                );
            }
            crate::Clause::OrderBy(order_by) => {
                clauses
                    .order_by
                    .extend(order_by.items.iter().filter_map(|item| {
                        let crate::FieldCheckResult::Column(column) =
                            catalog.check_field(table, &item.field.text)
                        else {
                            return None;
                        };
                        Some(OrderByPlan {
                            column: column.id,
                            direction: match &item.direction {
                                crate::SortDirectionExpr::Static(crate::SortDirection::Asc) => {
                                    SortDirectionPlan::Asc
                                }
                                crate::SortDirectionExpr::Static(crate::SortDirection::Desc) => {
                                    SortDirectionPlan::Desc
                                }
                                crate::SortDirectionExpr::Variable(variable) => {
                                    SortDirectionPlan::Variant {
                                        path: variable_path(
                                            selection_path,
                                            VariablePathContext {
                                                role: VariableRole::SortDirection,
                                                inferred_path: &[
                                                    column.name.clone(),
                                                    InputPathSegment::Direction
                                                        .as_ref()
                                                        .to_string(),
                                                ],
                                                anonymous_key: None,
                                            },
                                            variable_scope,
                                            variable.scope,
                                            variable.name.as_ref().map(|name| name.text.as_str()),
                                        ),
                                        variants: crate::SortDirection::ALL
                                            .iter()
                                            .map(|direction| {
                                                let label = direction.label().to_string();
                                                SqlVariantCase {
                                                    value: label.clone(),
                                                    text: label,
                                                }
                                            })
                                            .collect(),
                                    }
                                }
                            },
                        })
                    }));
            }
            crate::Clause::Limit(limit) => {
                clauses.limit = plan_u64_value(
                    selection_path,
                    variable_scope,
                    VariableRole::Limit,
                    InputPathSegment::Limit,
                    &limit.value,
                );
            }
            crate::Clause::Offset(offset) => {
                clauses.offset = plan_u64_value(
                    selection_path,
                    variable_scope,
                    VariableRole::Offset,
                    InputPathSegment::Offset,
                    &offset.value,
                );
            }
        }
    }
    clauses
}

fn plan_filter_expr(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    outer_current_table: Option<TableId>,
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    expr: &crate::Expr,
) -> Option<FilterExpr> {
    match expr {
        crate::Expr::Name(_) => None,
        crate::Expr::Variable(variable) => Some(FilterExpr::Parameter(SqlParameter {
            path: variable_path(
                selection_path,
                VariablePathContext {
                    role: VariableRole::WhereValue,
                    inferred_path: &[InputPathSegment::Value.as_ref().to_string()],
                    anonymous_key: None,
                },
                variable_scope,
                variable.scope,
                variable.name.as_ref().map(|name| name.text.as_str()),
            ),
        })),
        crate::Expr::Path(path) => {
            plan_filter_path(catalog, root_table, table, outer_current_table, path)
        }
        crate::Expr::Literal(literal) => Some(FilterExpr::Literal(match literal {
            crate::Literal::String { value, .. } => FilterLiteral::String(value.clone()),
            crate::Literal::Number { value, .. } => FilterLiteral::Number(value.clone()),
            crate::Literal::Bool { value, .. } => FilterLiteral::Bool(*value),
            crate::Literal::Null { .. } => FilterLiteral::Null,
        })),
        crate::Expr::Binary {
            left, op, right, ..
        } => {
            if let crate::Expr::Path(path) = left.as_ref()
                && is_comparison_operator(op)
                && let Some(field_path) = predicate_path(catalog, root_table, table, path)
            {
                let right = match right.as_ref() {
                    crate::Expr::Variable(variable) => FilterExpr::Parameter(SqlParameter {
                        path: variable_path(
                            selection_path,
                            VariablePathContext {
                                role: VariableRole::WhereValue,
                                inferred_path: &field_path,
                                anonymous_key: if variable.name.is_none()
                                    && matches!(op, crate::BinaryOperator::Variable(_))
                                {
                                    Some(InputPathSegment::Value.as_ref())
                                } else {
                                    None
                                },
                            },
                            variable_scope,
                            variable.scope,
                            variable.name.as_ref().map(|name| name.text.as_str()),
                        ),
                    }),
                    _ => plan_filter_expr(
                        catalog,
                        root_table,
                        table,
                        Some(table),
                        selection_path,
                        variable_scope,
                        right,
                    )?,
                };
                if let Some(filter) = relation_predicate_filter(
                    catalog,
                    table,
                    selection_path,
                    path,
                    op,
                    Some(field_path.join(".")),
                    right,
                    variable_scope,
                ) {
                    return Some(filter);
                }
            }
            if let (crate::Expr::Path(path), crate::Expr::Variable(variable)) =
                (left.as_ref(), right.as_ref())
                && let Some(field_path) = predicate_path(catalog, root_table, table, path)
            {
                let left = plan_filter_path(catalog, root_table, table, outer_current_table, path)?;
                let right = FilterExpr::Parameter(SqlParameter {
                    path: variable_path(
                        selection_path,
                        VariablePathContext {
                            role: VariableRole::WhereValue,
                            inferred_path: &field_path,
                            anonymous_key: if variable.name.is_none()
                                && matches!(op, crate::BinaryOperator::Variable(_))
                            {
                                Some(InputPathSegment::Value.as_ref())
                            } else {
                                None
                            },
                        },
                        variable_scope,
                        variable.scope,
                        variable.name.as_ref().map(|name| name.text.as_str()),
                    ),
                });
                return match op {
                    crate::BinaryOperator::Static(op) => Some(FilterExpr::Binary {
                        left: Box::new(left),
                        op: *op,
                        right: Box::new(right),
                    }),
                    crate::BinaryOperator::Variable(variable) => Some(FilterExpr::VariantBinary {
                        left: Box::new(left),
                        path: variable_path(
                            selection_path,
                            VariablePathContext {
                                role: VariableRole::ComparisonOperator,
                                inferred_path: &field_path,
                                anonymous_key: None,
                            },
                            variable_scope,
                            variable.scope,
                            variable.name.as_ref().map(|name| name.text.as_str()),
                        ),
                        variants: variable
                            .allowed
                            .iter()
                            .filter_map(|op| {
                                Some(SqlVariantCase {
                                    value: op.dsql_label()?.to_string(),
                                    text: postgres_operator(*op)?.to_string(),
                                })
                            })
                            .collect(),
                        right: Box::new(right),
                    }),
                };
            }
            let (left, left_path) = plan_filter_expr_with_path(
                catalog,
                root_table,
                table,
                outer_current_table,
                selection_path,
                variable_scope,
                left,
            )?;
            let (right, right_path) = plan_filter_expr_with_path(
                catalog,
                root_table,
                table,
                outer_current_table,
                selection_path,
                variable_scope,
                right,
            )?;
            match op {
                crate::BinaryOperator::Static(op) => Some(FilterExpr::Binary {
                    left: Box::new(left),
                    op: *op,
                    right: Box::new(right),
                }),
                crate::BinaryOperator::Variable(variable) => {
                    let inferred = path_parts(
                        left_path
                            .as_deref()
                            .or(right_path.as_deref())
                            .unwrap_or("operator"),
                    );
                    Some(FilterExpr::VariantBinary {
                        left: Box::new(left),
                        path: variable_path(
                            selection_path,
                            VariablePathContext {
                                role: VariableRole::ComparisonOperator,
                                inferred_path: &inferred,
                                anonymous_key: None,
                            },
                            variable_scope,
                            variable.scope,
                            variable.name.as_ref().map(|name| name.text.as_str()),
                        ),
                        variants: variable
                            .allowed
                            .iter()
                            .filter_map(|op| {
                                Some(SqlVariantCase {
                                    value: op.dsql_label()?.to_string(),
                                    text: postgres_operator(*op)?.to_string(),
                                })
                            })
                            .collect(),
                        right: Box::new(right),
                    })
                }
            }
        }
    }
}

fn is_comparison_operator(op: &crate::BinaryOperator) -> bool {
    match op {
        crate::BinaryOperator::Static(op) => is_comparison_op(*op),
        crate::BinaryOperator::Variable(_) => true,
    }
}

fn is_comparison_op(op: crate::BinaryOp) -> bool {
    matches!(
        op,
        crate::BinaryOp::Eq
            | crate::BinaryOp::Ne
            | crate::BinaryOp::Gt
            | crate::BinaryOp::Ge
            | crate::BinaryOp::Lt
            | crate::BinaryOp::Le
            | crate::BinaryOp::Like
    )
}

fn plan_filter_expr_with_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    outer_current_table: Option<TableId>,
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    expr: &crate::Expr,
) -> Option<(FilterExpr, Option<String>)> {
    match expr {
        crate::Expr::Path(path) => {
            let field_path = predicate_path(catalog, root_table, table, path);
            plan_filter_expr(
                catalog,
                root_table,
                table,
                outer_current_table,
                selection_path,
                variable_scope,
                expr,
            )
            .map(|expr| (expr, field_path.map(|parts| parts.join("."))))
        }
        crate::Expr::Variable(variable) => {
            let inferred = [InputPathSegment::Value.as_ref().to_string()];
            Some((
                FilterExpr::Parameter(SqlParameter {
                    path: variable_path(
                        selection_path,
                        VariablePathContext {
                            role: VariableRole::WhereValue,
                            inferred_path: &inferred,
                            anonymous_key: None,
                        },
                        variable_scope,
                        variable.scope,
                        variable.name.as_ref().map(|name| name.text.as_str()),
                    ),
                }),
                None,
            ))
        }
        _ => plan_filter_expr(
            catalog,
            root_table,
            table,
            outer_current_table,
            selection_path,
            variable_scope,
            expr,
        )
        .map(|expr| (expr, None)),
    }
}

fn plan_filter_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    outer_current_table: Option<TableId>,
    path: &crate::ScopedPath,
) -> Option<FilterExpr> {
    if path.segments.len() != 1 {
        return None;
    }
    let scope = match path.scope {
        crate::PathScope::Current if outer_current_table.is_some() => {
            FilterColumnScope::OuterCurrent
        }
        crate::PathScope::Current => FilterColumnScope::Current,
        crate::PathScope::Root => FilterColumnScope::Root,
        crate::PathScope::Parent => return None,
    };
    let source_table = match path.scope {
        crate::PathScope::Current => outer_current_table.unwrap_or(table),
        crate::PathScope::Root => root_table,
        crate::PathScope::Parent => return None,
    };
    let crate::FieldCheckResult::Column(column) =
        catalog.check_field(source_table, &path.segments[0].field_ref())
    else {
        return None;
    };
    Some(FilterExpr::Column {
        scope,
        column: column.id,
    })
}

fn relation_predicate_filter(
    catalog: &Catalog,
    table: TableId,
    selection_path: &[String],
    path: &crate::ScopedPath,
    op: &crate::BinaryOperator,
    operator_path: Option<String>,
    right: FilterExpr,
    variable_scope: &VariablePathScope,
) -> Option<FilterExpr> {
    if path.scope != crate::PathScope::Current || path.segments.len() < 2 {
        return None;
    }
    relation_predicate_segments(
        catalog,
        table,
        selection_path,
        &path.segments,
        op,
        operator_path,
        right,
        variable_scope,
    )
}

fn relation_predicate_segments(
    catalog: &Catalog,
    table: TableId,
    selection_path: &[String],
    segments: &[crate::ScopedPathSegment],
    op: &crate::BinaryOperator,
    operator_path: Option<String>,
    right: FilterExpr,
    variable_scope: &VariablePathScope,
) -> Option<FilterExpr> {
    if segments.len() < 2 {
        return None;
    }
    let crate::FieldCheckResult::Relation(relation) =
        catalog.check_field(table, &segments[0].field_ref())
    else {
        return None;
    };
    let filter = if segments.len() == 2 {
        let crate::FieldCheckResult::Column(column) =
            catalog.check_field(relation.table.id, &segments[1].field_ref())
        else {
            return None;
        };
        let left = FilterExpr::Column {
            scope: FilterColumnScope::Current,
            column: column.id,
        };
        match op {
            crate::BinaryOperator::Static(op) => FilterExpr::Binary {
                left: Box::new(left),
                op: *op,
                right: Box::new(right),
            },
            crate::BinaryOperator::Variable(variable) => {
                let inferred = operator_path
                    .map_or_else(|| vec![segments[1].field_ref()], |path| path_parts(&path));
                FilterExpr::VariantBinary {
                    left: Box::new(left),
                    path: variable_path(
                        selection_path,
                        VariablePathContext {
                            role: VariableRole::ComparisonOperator,
                            inferred_path: &inferred,
                            anonymous_key: None,
                        },
                        variable_scope,
                        variable.scope,
                        variable.name.as_ref().map(|name| name.text.as_str()),
                    ),
                    variants: variable
                        .allowed
                        .iter()
                        .filter_map(|op| {
                            Some(SqlVariantCase {
                                value: op.dsql_label()?.to_string(),
                                text: postgres_operator(*op)?.to_string(),
                            })
                        })
                        .collect(),
                    right: Box::new(right),
                }
            }
        }
    } else {
        relation_predicate_segments(
            catalog,
            relation.table.id,
            selection_path,
            &segments[1..],
            op,
            operator_path,
            right,
            variable_scope,
        )?
    };
    Some(FilterExpr::Exists {
        foreign_key: relation.foreign_key.id,
        table: relation.table.id,
        filter: Box::new(filter),
    })
}

fn plan_u64_value(
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    role: VariableRole,
    inferred_key: InputPathSegment,
    expr: &crate::Expr,
) -> Option<SqlValue> {
    match expr {
        crate::Expr::Literal(crate::Literal::Number { value, .. }) => {
            value.parse().ok().map(SqlValue::Literal)
        }
        crate::Expr::Variable(variable) => Some(SqlValue::Parameter(SqlParameter {
            path: variable_path(
                selection_path,
                VariablePathContext {
                    role,
                    inferred_path: &[inferred_key.as_ref().to_string()],
                    anonymous_key: None,
                },
                variable_scope,
                variable.scope,
                variable.name.as_ref().map(|name| name.text.as_str()),
            ),
        })),
        _ => None,
    }
}

fn predicate_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    path: &crate::ScopedPath,
) -> Option<Vec<String>> {
    let mut current_table = match path.scope {
        crate::PathScope::Current => table,
        crate::PathScope::Root => root_table,
        crate::PathScope::Parent => return None,
    };
    let (last, relations) = path.segments.split_last()?;
    let mut field_path = Vec::new();
    for relation_ref in relations {
        let field_ref = relation_ref.field_ref();
        let crate::FieldCheckResult::Relation(relation) =
            catalog.check_field(current_table, &field_ref)
        else {
            return None;
        };
        field_path.push(field_ref);
        current_table = relation.table.id;
    }
    let field_ref = last.field_ref();
    let crate::FieldCheckResult::Column(_) = catalog.check_field(current_table, &field_ref) else {
        return None;
    };
    field_path.push(field_ref);
    Some(field_path)
}

fn path_parts(path: &str) -> Vec<String> {
    path.split('.').map(ToString::to_string).collect()
}

fn postgres_operator(op: crate::BinaryOp) -> Option<&'static str> {
    match op {
        crate::BinaryOp::Eq => Some("="),
        crate::BinaryOp::Ne => Some("!="),
        crate::BinaryOp::Gt => Some(">"),
        crate::BinaryOp::Ge => Some(">="),
        crate::BinaryOp::Lt => Some("<"),
        crate::BinaryOp::Le => Some("<="),
        crate::BinaryOp::Like => Some("like"),
        crate::BinaryOp::And | crate::BinaryOp::Or => None,
    }
}

fn response_key(selection: &Selection) -> String {
    selection.alias.as_ref().map_or_else(
        || unqualified_name(&selection.name.text).to_string(),
        |alias| alias.text.clone(),
    )
}

fn unqualified_name(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_source;

    fn plan(source: &str) -> PlannedFile {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        plan_file(&parsed.source_file)
    }

    #[test]
    fn plans_scalar_projections_and_nested_relations() {
        let planned = plan("query Q { public.users { id name posts { title } } }");

        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);
        assert_eq!(planned.queries.len(), 1);
        assert_eq!(planned.queries[0].selections.items.len(), 3);
        assert!(matches!(
            &planned.queries[0].selections.items[0],
            SelectionPlanItem::Projection(Projection { output_name, .. }) if output_name == "id"
        ));
        assert!(matches!(
            &planned.queries[0].selections.items[2],
            SelectionPlanItem::Relation(NestedRelation { output_name, .. }) if output_name == "posts"
        ));
    }
}
