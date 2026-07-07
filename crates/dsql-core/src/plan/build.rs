//! The facts-to-plan builder.
//!
//! One plan per query-root selection: projections and nested relations from
//! the checked fact tree, filters lowered from expression trees (multi-step
//! relation predicates become `EXISTS` subqueries), variables becoming SQL
//! parameters with the same structured paths the variables stage infers.
//! Runs per definition, gated on [`PlanDemand`].

use bowl::{Bowl, Commands, DerivedFrom, Entity, Query, With};

use super::types::{
    FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, NestedRelation, OrderByPlan,
    Projection, QueryPlan, QueryPlanFact, SelectionClauses, SelectionPlan, SelectionPlanItem,
    SortDirectionPlan, SqlParameter, SqlValue, SqlVariantCase,
};
use crate::catalog::{
    Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, TableId, TableRef, TableResolution,
};
use crate::entities::clause::{ClauseFact, OrderDirection};
use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::expression::{BinaryOp, Expr, LiteralValue, PathAnchor, PathSegment, VariableRef};
use crate::entities::field_selection::{FieldSel, SelectionTree, TreeViews};
use crate::entities::variable::VariableRole;
use crate::entities::variable_path::{
    InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope, variable_path,
};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, NodeKey, PlanDemand,
    Severity, emit_diagnostic,
};

/// Registers the planning stage. A cross-entity stage system like
/// `generate_ast`: it walks the whole checked fact tree per definition.
pub async fn register_planning(bowl: &Bowl) {
    bowl.add_system(plan_queries).await;
}

/// Plans every root selection of each query definition. Root spreads are
/// skipped, unresolved
/// roots produce plan diagnostics, and fragment spreads below the root
/// splice the fragment's items in with an enveloped variable scope.
async fn plan_queries(
    _: Query<Entity, With<PlanDemand>>,
    defs: Query<(Entity, &DefDecl, &NodeKey, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (def_entity, decl, def_key, file) = defs.item();
    if decl.kind != DefKind::Query {
        return;
    }
    let (catalog_entity, snapshot) = catalog.item();

    let tree = SelectionTree::collect(&views, file.0);
    let mut planner = Planner {
        tree: &tree,
        catalog: snapshot.catalog(),
    };
    let mut diagnostics = Vec::new();

    let roots: Vec<_> = tree
        .fields_under(*def_key)
        .map(|(_, field, key, _)| (*field, *key))
        .collect();
    for (field, key) in roots {
        match planner
            .catalog
            .resolve_table_ref_for(TableRef::parse(&field.name))
        {
            TableResolution::Found(table) => {
                let table_id = table.id;
                let table_name = table.name.clone();
                let selection_path = vec![response_key(field)];
                let variable_scope = VariablePathScope::operation();
                let clauses = planner.plan_clauses(
                    table_id,
                    table_id,
                    &selection_path,
                    &variable_scope,
                    key,
                );
                if let Some(selections) = planner.plan_selection_set(
                    table_id,
                    table_id,
                    &clauses,
                    SelectionPath::body(selection_path),
                    &variable_scope,
                    key,
                    &mut Vec::new(),
                    &mut diagnostics,
                ) {
                    let plan = QueryPlan {
                        root: table_id,
                        output_name: field
                            .alias
                            .clone()
                            .unwrap_or(table_name),
                        clauses,
                        selections,
                    };
                    commands.insert((
                        DerivedFrom::many([def_entity, catalog_entity]),
                        BelongsToFile(file.0),
                        QueryPlanFact(plan),
                    ));
                }
            }
            TableResolution::NotFound { reference } => diagnostics.push((
                field.name_span,
                DiagnosticCode::TableNotFound,
                format!("table `{reference}` not found"),
            )),
            TableResolution::Ambiguous {
                reference,
                candidates,
            } => {
                let candidates: Vec<String> = candidates
                    .iter()
                    .map(|key| format!("{}.{}", key.schema, key.table))
                    .collect();
                diagnostics.push((
                    field.name_span,
                    DiagnosticCode::AmbiguousTable,
                    format!(
                        "table `{reference}` is ambiguous; use an alias with a schema-qualified name ({})",
                        candidates.join(", ")
                    ),
                ));
            }
        }
    }

    for (span, code, message) in diagnostics {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([def_entity, catalog_entity]),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Plan,
                code,
                message,
            },
        );
    }
}

type PlanDiagnostics = Vec<(crate::facts::Span, DiagnosticCode, String)>;

struct Planner<'a> {
    tree: &'a SelectionTree<'a>,
    catalog: &'a Catalog,
}

impl Planner<'_> {
    #[expect(clippy::too_many_arguments, reason = "recursion threads the whole walk state")]
    fn plan_selection_set(
        &mut self,
        root_table: TableId,
        table: TableId,
        clauses: &SelectionClauses,
        selection_path: SelectionPath,
        variable_scope: &VariablePathScope,
        parent: NodeKey,
        visiting: &mut Vec<String>,
        diagnostics: &mut PlanDiagnostics,
    ) -> Option<SelectionPlan> {
        let mut items = Vec::new();

        // Facts carry no sibling order between fields and spreads beyond
        // their spans, so source order is restored by merging on span order.
        enum Child<'a> {
            Field(&'a FieldSel, NodeKey),
            Spread(String),
        }
        let mut children: Vec<(usize, Child<'_>)> = self
            .tree
            .fields_under(parent)
            .map(|(_, field, key, _)| (field.span.start, Child::Field(field, *key)))
            .collect();
        children.extend(
            self.tree
                .spreads_under(parent)
                .map(|(_, spread, _, _)| {
                    (spread.span.start, Child::Spread(spread.name.clone()))
                }),
        );
        children.sort_by_key(|(start, _)| *start);

        for (_, child) in children {
            match child {
                Child::Spread(name) => {
                    let Some((_, _, _, fragment_key)) =
                        self.tree.fragment_named(&name).copied()
                    else {
                        continue;
                    };
                    // Planning is demand-driven and runs regardless of check
                    // status, so cyclic spreads must be guarded against here
                    // rather than trusting checks to have rejected them.
                    if visiting.contains(&name) {
                        continue;
                    }
                    visiting.push(name.clone());
                    if let Some(fragment_plan) = self.plan_selection_set(
                        root_table,
                        table,
                        &SelectionClauses::default(),
                        SelectionPath::fragment_root(),
                        &variable_scope.for_fragment_spread(&selection_path, &name),
                        fragment_key,
                        visiting,
                        diagnostics,
                    ) {
                        items.extend(fragment_plan.items);
                    }
                    visiting.pop();
                }
                Child::Field(field, key) => {
                    let reference = FieldRef {
                        target: TableRef::parse(&field.name),
                        selector: field.relation_path.as_deref(),
                    };
                    match self.catalog.check_field_ref(table, reference) {
                        FieldCheckResult::Column(column) => {
                            if !field.nested {
                                items.push(SelectionPlanItem::Projection(Projection {
                                    column: column.id,
                                    output_name: field
                                        .alias
                                        .clone()
                                        .unwrap_or_else(|| column.name.clone()),
                                }));
                            }
                        }
                        FieldCheckResult::Relation(relation) => {
                            let relation_table = relation.table.id;
                            let relation_name = relation.name.to_string();
                            let foreign_key = relation.foreign_key.id;
                            let child_path = selection_path.relation_child_path(
                                field.alias.clone().unwrap_or_else(|| relation_name.clone()),
                            );
                            let child_clauses = self.plan_clauses(
                                root_table,
                                relation_table,
                                &child_path,
                                variable_scope,
                                key,
                            );
                            if let Some(nested) = self.plan_selection_set(
                                root_table,
                                relation_table,
                                &child_clauses,
                                SelectionPath::body(child_path),
                                variable_scope,
                                key,
                                visiting,
                                diagnostics,
                            ) {
                                items.push(SelectionPlanItem::Relation(NestedRelation {
                                    relation_name: reference.display_text(),
                                    output_name: field
                                        .alias
                                        .clone()
                                        .unwrap_or(relation_name),
                                    table: relation_table,
                                    foreign_key,
                                    selections: Box::new(nested),
                                }));
                            }
                        }
                        FieldCheckResult::NotFound => {}
                        FieldCheckResult::AmbiguousRelation {
                            reference,
                            candidates,
                        } => diagnostics.push((
                            field.name_span,
                            DiagnosticCode::AmbiguousRelation,
                            format!(
                                "relation `{reference}` has multiple foreign-key paths; use one of: {}",
                                candidates.join(", ")
                            ),
                        )),
                    }
                }
            }
        }

        Some(SelectionPlan {
            table,
            clauses: clauses.clone(),
            items,
        })
    }

    fn plan_clauses(
        &mut self,
        root_table: TableId,
        table: TableId,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        field_key: NodeKey,
    ) -> SelectionClauses {
        let mut clauses = SelectionClauses::default();
        let field_clauses: Vec<ClauseFact> = self
            .tree
            .clauses_under(field_key)
            .map(|(_, clause, _, _)| (*clause).clone())
            .collect();
        for clause in field_clauses {
            match clause {
                ClauseFact::Where { expr } => {
                    clauses.filter = self.plan_filter_expr(
                        root_table,
                        table,
                        None,
                        selection_path,
                        variable_scope,
                        &expr,
                    );
                }
                ClauseFact::OrderBy { items } => {
                    clauses.order_by.extend(items.iter().filter_map(|item| {
                        let reference = FieldRef {
                            target: TableRef::parse(&item.field),
                            selector: None,
                        };
                        let FieldCheckResult::Column(column) =
                            self.catalog.check_field_ref(table, reference)
                        else {
                            return None;
                        };
                        Some(OrderByPlan {
                            column: column.id,
                            direction: match &item.direction {
                                Some(OrderDirection::Asc) | None => SortDirectionPlan::Asc,
                                Some(OrderDirection::Desc) => SortDirectionPlan::Desc,
                                Some(OrderDirection::Variable(variable)) => {
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
                                            variable.sigil,
                                            variable.name.as_deref(),
                                        ),
                                        variants: ["asc", "desc"]
                                            .iter()
                                            .map(|label| SqlVariantCase {
                                                value: (*label).to_string(),
                                                text: (*label).to_string(),
                                            })
                                            .collect(),
                                    }
                                }
                            },
                        })
                    }));
                }
                ClauseFact::Limit { expr } => {
                    clauses.limit = plan_u64_value(
                        selection_path,
                        variable_scope,
                        VariableRole::Limit,
                        InputPathSegment::Limit,
                        &expr,
                    );
                }
                ClauseFact::Offset { expr } => {
                    clauses.offset = plan_u64_value(
                        selection_path,
                        variable_scope,
                        VariableRole::Offset,
                        InputPathSegment::Offset,
                        &expr,
                    );
                }
            }
        }
        clauses
    }

    fn plan_filter_expr(
        &mut self,
        root_table: TableId,
        table: TableId,
        outer_current_table: Option<TableId>,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        expr: &Expr,
    ) -> Option<FilterExpr> {
        match expr {
            Expr::Error { .. } => None,
            Expr::Variable { variable, .. } => Some(FilterExpr::Parameter(SqlParameter {
                path: variable_path(
                    selection_path,
                    VariablePathContext {
                        role: VariableRole::WhereValue,
                        inferred_path: &[InputPathSegment::Value.as_ref().to_string()],
                        anonymous_key: None,
                    },
                    variable_scope,
                    variable.sigil,
                    variable.name.as_deref(),
                ),
            })),
            Expr::Path { .. } => {
                self.plan_filter_path(root_table, table, outer_current_table, expr)
            }
            Expr::Literal { value, .. } => Some(FilterExpr::Literal(match value {
                LiteralValue::String(value) => FilterLiteral::String(value.clone()),
                LiteralValue::Number(value) => FilterLiteral::Number(value.clone()),
                LiteralValue::Bool(value) => FilterLiteral::Bool(*value),
                LiteralValue::Null => FilterLiteral::Null,
            })),
            Expr::Binary { op, lhs, rhs, .. } => {
                if let Expr::Path { .. } = lhs.as_ref()
                    && is_comparison_operator(op)
                    && let Some(field_path) = self.predicate_path(root_table, table, lhs)
                {
                    let right = match rhs.as_ref() {
                        Expr::Variable { variable, .. } => FilterExpr::Parameter(SqlParameter {
                            path: where_value_path(
                                selection_path,
                                variable_scope,
                                &field_path,
                                op,
                                variable,
                            ),
                        }),
                        _ => self.plan_filter_expr(
                            root_table,
                            table,
                            Some(table),
                            selection_path,
                            variable_scope,
                            rhs,
                        )?,
                    };
                    if let Some(filter) = self.relation_predicate_filter(
                        table,
                        selection_path,
                        lhs,
                        op,
                        Some(field_path.join(".")),
                        right,
                        variable_scope,
                    ) {
                        return Some(filter);
                    }
                }
                if let (path @ Expr::Path { .. }, Expr::Variable { variable, .. }) =
                    (lhs.as_ref(), rhs.as_ref())
                    && let Some(field_path) = self.predicate_path(root_table, table, path)
                {
                    let left =
                        self.plan_filter_path(root_table, table, outer_current_table, path)?;
                    let right = FilterExpr::Parameter(SqlParameter {
                        path: where_value_path(
                            selection_path,
                            variable_scope,
                            &field_path,
                            op,
                            variable,
                        ),
                    });
                    return self.binary_or_variant(
                        left,
                        op,
                        right,
                        selection_path,
                        variable_scope,
                        &field_path,
                    );
                }
                let (left, left_path) = self.plan_filter_expr_with_path(
                    root_table,
                    table,
                    outer_current_table,
                    selection_path,
                    variable_scope,
                    lhs,
                )?;
                let (right, right_path) = self.plan_filter_expr_with_path(
                    root_table,
                    table,
                    outer_current_table,
                    selection_path,
                    variable_scope,
                    rhs,
                )?;
                let inferred = path_parts(
                    left_path
                        .as_deref()
                        .or(right_path.as_deref())
                        .unwrap_or("operator"),
                );
                self.binary_or_variant(left, op, right, selection_path, variable_scope, &inferred)
            }
        }
    }

    /// A concrete binary filter, or a variant filter when the operator is
    /// an operator variable.
    fn binary_or_variant(
        &mut self,
        left: FilterExpr,
        op: &BinaryOp,
        right: FilterExpr,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        inferred_path: &[String],
    ) -> Option<FilterExpr> {
        match op {
            BinaryOp::Comparison(op) => Some(FilterExpr::Binary {
                left: Box::new(left),
                op: FilterOp::from(*op),
                right: Box::new(right),
            }),
            BinaryOp::And => Some(FilterExpr::Binary {
                left: Box::new(left),
                op: FilterOp::And,
                right: Box::new(right),
            }),
            BinaryOp::Or => Some(FilterExpr::Binary {
                left: Box::new(left),
                op: FilterOp::Or,
                right: Box::new(right),
            }),
            BinaryOp::Variable(variable) => Some(FilterExpr::VariantBinary {
                left: Box::new(left),
                path: variable_path(
                    selection_path,
                    VariablePathContext {
                        role: VariableRole::ComparisonOperator,
                        inferred_path,
                        anonymous_key: None,
                    },
                    variable_scope,
                    variable.sigil,
                    variable.name.as_deref(),
                ),
                variants: operator_variants(variable),
                right: Box::new(right),
            }),
        }
    }

    fn plan_filter_expr_with_path(
        &mut self,
        root_table: TableId,
        table: TableId,
        outer_current_table: Option<TableId>,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        expr: &Expr,
    ) -> Option<(FilterExpr, Option<String>)> {
        match expr {
            Expr::Path { .. } => {
                let field_path = self.predicate_path(root_table, table, expr);
                self.plan_filter_expr(
                    root_table,
                    table,
                    outer_current_table,
                    selection_path,
                    variable_scope,
                    expr,
                )
                .map(|expr| (expr, field_path.map(|parts| parts.join("."))))
            }
            Expr::Variable { variable, .. } => {
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
                            variable.sigil,
                            variable.name.as_deref(),
                        ),
                    }),
                    None,
                ))
            }
            _ => self
                .plan_filter_expr(
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
        &mut self,
        root_table: TableId,
        table: TableId,
        outer_current_table: Option<TableId>,
        path: &Expr,
    ) -> Option<FilterExpr> {
        let Expr::Path {
            anchor, segments, ..
        } = path
        else {
            return None;
        };
        if segments.len() != 1 {
            return None;
        }
        let scope = match anchor {
            PathAnchor::Current if outer_current_table.is_some() => {
                FilterColumnScope::OuterCurrent
            }
            PathAnchor::Current => FilterColumnScope::Current,
            PathAnchor::Root => FilterColumnScope::Root,
            PathAnchor::Parent => return None,
        };
        let source_table = match anchor {
            PathAnchor::Current => outer_current_table.unwrap_or(table),
            PathAnchor::Root => root_table,
            PathAnchor::Parent => return None,
        };
        let FieldCheckResult::Column(column) = self
            .catalog
            .check_field_ref(source_table, segment_field_ref(&segments[0]))
        else {
            return None;
        };
        Some(FilterExpr::Column {
            scope,
            column: column.id,
        })
    }

    /// Multi-segment `Current`-scoped predicate paths become nested
    /// `EXISTS` subqueries stepping through each relation.
    #[expect(clippy::too_many_arguments, reason = "recursion threads the whole walk state")]
    fn relation_predicate_filter(
        &mut self,
        table: TableId,
        selection_path: &[String],
        path: &Expr,
        op: &BinaryOp,
        operator_path: Option<String>,
        right: FilterExpr,
        variable_scope: &VariablePathScope,
    ) -> Option<FilterExpr> {
        let Expr::Path {
            anchor: PathAnchor::Current,
            segments,
            ..
        } = path
        else {
            return None;
        };
        if segments.len() < 2 {
            return None;
        }
        self.relation_predicate_segments(
            table,
            selection_path,
            segments,
            op,
            operator_path,
            right,
            variable_scope,
        )
    }

    #[expect(clippy::too_many_arguments, reason = "recursion threads the whole walk state")]
    fn relation_predicate_segments(
        &mut self,
        table: TableId,
        selection_path: &[String],
        segments: &[PathSegment],
        op: &BinaryOp,
        operator_path: Option<String>,
        right: FilterExpr,
        variable_scope: &VariablePathScope,
    ) -> Option<FilterExpr> {
        if segments.len() < 2 {
            return None;
        }
        let FieldCheckResult::Relation(relation) = self
            .catalog
            .check_field_ref(table, segment_field_ref(&segments[0]))
        else {
            return None;
        };
        let relation_table = relation.table.id;
        let relation_fk = relation.foreign_key.id;
        let filter = if segments.len() == 2 {
            let FieldCheckResult::Column(column) = self
                .catalog
                .check_field_ref(relation_table, segment_field_ref(&segments[1]))
            else {
                return None;
            };
            let left = FilterExpr::Column {
                scope: FilterColumnScope::Current,
                column: column.id,
            };
            let inferred = operator_path.map_or_else(
                || vec![segment_display(&segments[1])],
                |path| path_parts(&path),
            );
            self.binary_or_variant(left, op, right, selection_path, variable_scope, &inferred)?
        } else {
            self.relation_predicate_segments(
                relation_table,
                selection_path,
                &segments[1..],
                op,
                operator_path,
                right,
                variable_scope,
            )?
        };
        Some(FilterExpr::Exists {
            foreign_key: relation_fk,
            table: relation_table,
            filter: Box::new(filter),
        })
    }

    fn predicate_path(
        &self,
        root_table: TableId,
        table: TableId,
        path: &Expr,
    ) -> Option<Vec<String>> {
        let Expr::Path {
            anchor, segments, ..
        } = path
        else {
            return None;
        };
        let mut current_table = match anchor {
            PathAnchor::Current => table,
            PathAnchor::Root => root_table,
            PathAnchor::Parent => return None,
        };
        let (last, relations) = segments.split_last()?;
        let mut field_path = Vec::new();
        for segment in relations {
            let FieldCheckResult::Relation(relation) = self
                .catalog
                .check_field_ref(current_table, segment_field_ref(segment))
            else {
                return None;
            };
            field_path.push(segment_display(segment));
            current_table = relation.table.id;
        }
        let FieldCheckResult::Column(_) = self
            .catalog
            .check_field_ref(current_table, segment_field_ref(last))
        else {
            return None;
        };
        field_path.push(segment_display(last));
        Some(field_path)
    }
}

/// The parameter path of a where-value variable bound to `field_path`.
fn where_value_path(
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    field_path: &[String],
    op: &BinaryOp,
    variable: &VariableRef,
) -> String {
    variable_path(
        selection_path,
        VariablePathContext {
            role: VariableRole::WhereValue,
            inferred_path: field_path,
            anonymous_key: if variable.name.is_none() && matches!(op, BinaryOp::Variable(_)) {
                Some(InputPathSegment::Value.as_ref())
            } else {
                None
            },
        },
        variable_scope,
        variable.sigil,
        variable.name.as_deref(),
    )
}

fn is_comparison_operator(op: &BinaryOp) -> bool {
    matches!(op, BinaryOp::Comparison(_) | BinaryOp::Variable(_))
}

fn operator_variants(variable: &VariableRef) -> Vec<SqlVariantCase> {
    variable
        .operators
        .iter()
        .flatten()
        .filter_map(|op| {
            let op = FilterOp::from(*op);
            Some(SqlVariantCase {
                value: op.dsql_label()?.to_string(),
                text: op.postgres_text()?.to_string(),
            })
        })
        .collect()
}

fn plan_u64_value(
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    role: VariableRole,
    inferred_key: InputPathSegment,
    expr: &Expr,
) -> Option<SqlValue> {
    match expr {
        Expr::Literal {
            value: LiteralValue::Number(value),
            ..
        } => value.parse().ok().map(SqlValue::Literal),
        Expr::Variable { variable, .. } => Some(SqlValue::Parameter(SqlParameter {
            path: variable_path(
                selection_path,
                VariablePathContext {
                    role,
                    inferred_path: &[inferred_key.as_ref().to_string()],
                    anonymous_key: None,
                },
                variable_scope,
                variable.sigil,
                variable.name.as_deref(),
            ),
        })),
        _ => None,
    }
}

fn segment_field_ref(segment: &PathSegment) -> FieldRef<'_> {
    FieldRef {
        target: TableRef::parse(&segment.name),
        selector: segment.relation_path.as_deref(),
    }
}

fn segment_display(segment: &PathSegment) -> String {
    segment_field_ref(segment).display_text()
}

fn path_parts(path: &str) -> Vec<String> {
    path.split('.').map(ToString::to_string).collect()
}

/// Output key of a selection: alias, or the object name of its target.
fn response_key(selection: &FieldSel) -> String {
    selection
        .alias
        .clone()
        .unwrap_or_else(|| TableRef::parse(&selection.name).name.to_string())
}
