use super::{
    FilterColumnScope, FilterExpr, FilterLiteral, NestedRelation, OrderByPlan, PlannedFile,
    Projection, QueryPlan, SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan,
    SqlParameter, SqlValue, SqlVariantCase,
};
use crate::{
    catalog::{Catalog, FieldCheckResult, TableId, TableKey, TableResolution},
    definition::{DefinitionResolver, FragmentMap, QueryRecord, extract_definitions},
    syntax::{
        Definition, Diagnostic, DiagnosticCode, DiagnosticSource, Selection, SelectionKind,
        Severity, SourceFile,
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
                    let clauses =
                        plan_clauses(catalog, table.id, table.id, &selection_path, selection);
                    if let Some(selections) = plan_selection_set(
                        catalog,
                        &resolver,
                        table.id,
                        table.id,
                        &clauses,
                        &selection_path,
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
                TableResolution::NotFound { reference } => diagnostics.push(planner_diagnostic(
                    selection.name.range,
                    DiagnosticCode::TableNotFound,
                    format!("table `{reference}` not found"),
                )),
                TableResolution::Ambiguous {
                    reference,
                    candidates,
                } => diagnostics.push(planner_diagnostic(
                    selection.name.range,
                    DiagnosticCode::AmbiguousTable,
                    format!(
                        "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                        reference,
                        format_table_candidates(&candidates)
                    ),
                )),
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
            diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::UnknownFragment,
                format!("fragment `{}` not found", selection.name.text),
            ));
            continue;
        }
        match catalog.resolve_table_ref(&selection.name.text) {
            TableResolution::Found(table) => {
                let selection_path = vec![response_key(selection)];
                let clauses = plan_clauses(catalog, table.id, table.id, &selection_path, selection);
                if let Some(selections) = plan_selection_set(
                    catalog,
                    resolver,
                    table.id,
                    table.id,
                    &clauses,
                    &selection_path,
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
            TableResolution::NotFound { reference } => diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::TableNotFound,
                format!("table `{reference}` not found"),
            )),
            TableResolution::Ambiguous {
                reference,
                candidates,
            } => diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::AmbiguousTable,
                format!(
                    "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    reference,
                    format_table_candidates(&candidates)
                ),
            )),
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    PlannedFile {
        queries,
        diagnostics,
    }
}

fn plan_selection_set(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    root_table: TableId,
    table: TableId,
    clauses: &SelectionClauses,
    selection_path: &[String],
    selections: &[Selection],
    diagnostics: &mut Vec<Diagnostic>,
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
                    selection_path,
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
                let mut child_path = selection_path.to_vec();
                child_path.push(
                    selection
                        .alias
                        .as_ref()
                        .map_or_else(|| relation.name.to_string(), |alias| alias.text.clone()),
                );
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
                        selection,
                    ),
                    &child_path,
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
            } => diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::AmbiguousRelation,
                format!(
                    "relation `{}` has multiple foreign-key paths; use one of: {}",
                    reference,
                    candidates.join(", ")
                ),
            )),
        }
    }
    Some(SelectionPlan {
        table,
        clauses: clauses.clone(),
        items,
    })
}

fn plan_clauses(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection_path: &[String],
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
                                                role: VariablePathRole::SortDirection,
                                                inferred_path: &[
                                                    column.name.clone(),
                                                    "direction".to_string(),
                                                ],
                                                anonymous_key: None,
                                            },
                                            variable.scope,
                                            variable.name.as_ref().map(|name| name.text.as_str()),
                                        ),
                                        variants: crate::SortDirection::ALL
                                            .iter()
                                            .map(|direction| SqlVariantCase {
                                                value: direction.label().to_string(),
                                                text: direction.label().to_string(),
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
                    VariablePathRole::Limit,
                    "limit",
                    &limit.value,
                );
            }
            crate::Clause::Offset(offset) => {
                clauses.offset = plan_u64_value(
                    selection_path,
                    VariablePathRole::Offset,
                    "offset",
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
    expr: &crate::Expr,
) -> Option<FilterExpr> {
    match expr {
        crate::Expr::Name(_) => None,
        crate::Expr::Variable(variable) => Some(FilterExpr::Parameter(SqlParameter {
            path: variable_path(
                selection_path,
                VariablePathContext {
                    role: VariablePathRole::WhereValue,
                    inferred_path: &["value".to_string()],
                    anonymous_key: None,
                },
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
                                role: VariablePathRole::WhereValue,
                                inferred_path: &field_path,
                                anonymous_key: if variable.name.is_none()
                                    && matches!(op, crate::BinaryOperator::Variable(_))
                                {
                                    Some("value")
                                } else {
                                    None
                                },
                            },
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
                            role: VariablePathRole::WhereValue,
                            inferred_path: &field_path,
                            anonymous_key: if variable.name.is_none()
                                && matches!(op, crate::BinaryOperator::Variable(_))
                            {
                                Some("value")
                            } else {
                                None
                            },
                        },
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
                                role: VariablePathRole::ComparisonOperator,
                                inferred_path: &field_path,
                                anonymous_key: None,
                            },
                            variable.scope,
                            variable.name.as_ref().map(|name| name.text.as_str()),
                        ),
                        variants: variable
                            .allowed
                            .iter()
                            .filter_map(|op| {
                                Some(SqlVariantCase {
                                    value: op.label()?.to_string(),
                                    text: postgres_operator(*op).to_string(),
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
                left,
            )?;
            let (right, right_path) = plan_filter_expr_with_path(
                catalog,
                root_table,
                table,
                outer_current_table,
                selection_path,
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
                                role: VariablePathRole::ComparisonOperator,
                                inferred_path: &inferred,
                                anonymous_key: None,
                            },
                            variable.scope,
                            variable.name.as_ref().map(|name| name.text.as_str()),
                        ),
                        variants: variable
                            .allowed
                            .iter()
                            .filter_map(|op| {
                                Some(SqlVariantCase {
                                    value: op.label()?.to_string(),
                                    text: postgres_operator(*op).to_string(),
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
                expr,
            )
            .map(|expr| (expr, field_path.map(|parts| parts.join("."))))
        }
        crate::Expr::Variable(variable) => {
            let inferred = ["value".to_string()];
            Some((
                FilterExpr::Parameter(SqlParameter {
                    path: variable_path(
                        selection_path,
                        VariablePathContext {
                            role: VariablePathRole::WhereValue,
                            inferred_path: &inferred,
                            anonymous_key: None,
                        },
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
                            role: VariablePathRole::ComparisonOperator,
                            inferred_path: &inferred,
                            anonymous_key: None,
                        },
                        variable.scope,
                        variable.name.as_ref().map(|name| name.text.as_str()),
                    ),
                    variants: variable
                        .allowed
                        .iter()
                        .filter_map(|op| {
                            Some(SqlVariantCase {
                                value: op.label()?.to_string(),
                                text: postgres_operator(*op).to_string(),
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
    role: VariablePathRole,
    inferred_key: &str,
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
                    inferred_path: &[inferred_key.to_string()],
                    anonymous_key: None,
                },
                variable.scope,
                variable.name.as_ref().map(|name| name.text.as_str()),
            ),
        })),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum VariablePathRole {
    WhereValue,
    ComparisonOperator,
    SortDirection,
    Limit,
    Offset,
}

struct VariablePathContext<'a> {
    role: VariablePathRole,
    inferred_path: &'a [String],
    anonymous_key: Option<&'a str>,
}

fn variable_path(
    selection_path: &[String],
    context: VariablePathContext<'_>,
    scope: crate::VariableScope,
    name: Option<&str>,
) -> String {
    let key = name.map_or_else(
        || {
            if let Some(key) = context.anonymous_key {
                key.to_string()
            } else if matches!(context.role, VariablePathRole::ComparisonOperator) {
                "op".to_string()
            } else {
                context.inferred_path.last().cloned().unwrap_or_default()
            }
        },
        ToString::to_string,
    );
    match scope {
        crate::VariableScope::Structured => {
            let mut parts = vec!["input".to_string()];
            parts.extend(selection_path.iter().cloned());
            parts.push("clause".to_string());
            parts.push(
                match context.role {
                    VariablePathRole::WhereValue | VariablePathRole::ComparisonOperator => "where",
                    VariablePathRole::SortDirection => "order_by",
                    VariablePathRole::Limit => "limit",
                    VariablePathRole::Offset => "offset",
                }
                .to_string(),
            );
            if matches!(
                context.role,
                VariablePathRole::WhereValue
                    | VariablePathRole::ComparisonOperator
                    | VariablePathRole::SortDirection
            ) {
                parts.extend(context.inferred_path.iter().cloned());
            }
            if name.is_some()
                || context.anonymous_key.is_some()
                || matches!(context.role, VariablePathRole::ComparisonOperator)
            {
                parts.push(key);
            }
            parts.join(".")
        }
        crate::VariableScope::TopLevel => format!("params.{key}"),
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

fn postgres_operator(op: crate::BinaryOp) -> &'static str {
    match op {
        crate::BinaryOp::Eq => "=",
        crate::BinaryOp::Ne => "!=",
        crate::BinaryOp::Gt => ">",
        crate::BinaryOp::Ge => ">=",
        crate::BinaryOp::Lt => "<",
        crate::BinaryOp::Le => "<=",
        crate::BinaryOp::Like => "like",
        crate::BinaryOp::And | crate::BinaryOp::Or => "",
    }
}

fn planner_diagnostic(
    range: crate::TextRange,
    code: DiagnosticCode,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        range,
        severity: Severity::Error,
        code,
        message: message.into(),
        source: DiagnosticSource::Check,
    }
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
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
