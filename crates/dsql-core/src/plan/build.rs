//! The facts-to-plan builder.
//!
//! One plan per query-root selection: projections and nested relations from
//! the checked fact tree, filters lowered from expression trees (multi-step
//! relation predicates become `EXISTS` subqueries), variables becoming SQL
//! parameters with the same structured paths the variables stage infers.
//! Runs per definition, gated on [`PlanDemand`].

use crate::entities::expansion::{ExpandedSpread, SpreadExpansion};
use crate::resolution::{PathTerminal, ResolvedClause, index_resolved_clauses};
use crate::schema::dsql_schema;
use bowl::{Commands, DerivedFrom, Entity, Query, Registrar, SystemExt, SystemParam, View, With};

use super::types::{
    AggregateGroupProjection, AggregatePlan, AggregateProjection, CollectionPlan,
    CollectionResultPlan, FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, FragmentPlanFact,
    NestedRelation, OperationSeed, OrderByPlan, Projection, QueryPlan, QueryPlanFact,
    SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan, SpreadUse, SqlParameter,
    SqlValue, SqlVariantCase,
};
use crate::catalog::{
    Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, TableId, TableRef, TableResolution,
};
use crate::entities::aggregate::ResolvedAggregate;
use crate::entities::clause::{ClauseFact, OrderDirection};
use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::expression::{BinaryOp, Expr, LiteralValue, PathAnchor, VariableRef};
use crate::entities::field_selection::{FieldSel, SelectionTree, TreeViews};
use crate::entities::variable::VariableRole;
use crate::entities::variable_path::{
    InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope, variable_path,
};
use crate::facts::{
    BelongsToFile, DefKey, DiagnosticCode, DiagnosticFacts, DiagnosticSource, PlanDemand, PlanKey,
    Severity, emit_diagnostic,
};
use crate::source::{ResolutionScope, ScopeImports};

/// Registers the planning stage. A cross-entity stage system like
/// `lower_syntax_facts`: it walks the whole checked fact tree per definition.
pub fn register_planning(reg: &mut Registrar<'_>) {
    // Views lowered facts ambiently: behind the Complete barrier.
    reg.system(plan_queries.run_during(bowl::Phase::Complete));
}

/// Plans every root selection of each query definition. Root spreads are
/// skipped, unresolved
/// roots produce plan diagnostics, and fragment spreads below the root
/// splice the fragment's items in with an enveloped variable scope.
#[expect(
    clippy::too_many_arguments,
    reason = "system parameters are the tracked join, not an API surface"
)]
async fn plan_queries(
    _: Query<Entity, With<PlanDemand>>,
    defs: Query<(Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    _index: Query<(Entity, &crate::entities::definition::DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    views: TreeViews<'_>,
    semantic_views: PlanSemanticViews<'_>,
    mut commands: Commands<(
        dsql_schema::QueryPlan,
        dsql_schema::FragmentPlan,
        dsql_schema::Diagnostic,
    )>,
) {
    let (def_entity, decl, file, scope) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (_, imports) = imports.item();

    let tree = SelectionTree::collect(&views);
    let resolved_clauses =
        index_resolved_clauses(semantic_views.clauses.iter().map(|(_, resolved)| resolved));
    let resolved_aggregates = semantic_views
        .aggregates
        .iter()
        .map(|(_, aggregate)| (aggregate.source, aggregate))
        .collect();
    let planner = Planner {
        tree: &tree,
        resolved_clauses: &resolved_clauses,
        resolved_aggregates: &resolved_aggregates,
        catalog: snapshot.catalog(),
        scope: &scope.0,
        imports,
    };
    let mut diagnostics = Vec::new();

    if decl.kind == DefKind::Fragment {
        plan_fragment_body(
            &planner,
            def_entity,
            decl,
            file.0,
            &scope.0,
            catalog_entity,
            &mut commands,
            &mut diagnostics,
        );
        emit_plan_diagnostics(
            diagnostics,
            def_entity,
            catalog_entity,
            file.0,
            &mut commands,
        );
        return;
    }

    let roots: Vec<_> = tree
        .fields_under(def_entity)
        .map(|(entity, field, _, _)| (*entity, *field))
        .collect();
    let root_count = roots.len();
    for (root_index, (root_entity, field)) in roots.into_iter().enumerate() {
        match planner
            .catalog
            .resolve_table_ref_for(TableRef::parse(&field.name))
        {
            TableResolution::Found(table) => {
                let table_id = table.id;
                let output_name = field.alias.clone().unwrap_or_else(|| table.name.clone());
                let mut selection_path = vec![field.output_key()];
                if field.flattened && field.has_transform() {
                    selection_path.push(InputPathSegment::Aggregate.as_ref().to_string());
                }
                let variable_scope = VariablePathScope::operation();
                let clauses =
                    planner.plan_clauses(table_id, &selection_path, &variable_scope, root_entity);
                let mut spreads = Vec::new();
                let mut walk = PlanWalk {
                    result_path: if field.flattened {
                        Vec::new()
                    } else {
                        vec![output_name.clone()]
                    },
                    spreads: &mut spreads,
                    expansion: &mut SpreadExpansion::new(
                        planner.tree,
                        planner.scope,
                        planner.imports,
                    ),
                    diagnostics: &mut diagnostics,
                };
                if let Some(result) = planner.plan_collection_result(
                    &mut walk,
                    table_id,
                    SelectionPath::body(selection_path),
                    &variable_scope,
                    root_entity,
                    field,
                ) {
                    let plan = QueryPlan {
                        output_name,
                        flattened: field.flattened,
                        collection: CollectionPlan {
                            table: table_id,
                            clauses,
                            result,
                        },
                    };
                    let plan_entity = commands.insert((
                        DerivedFrom::many([def_entity, catalog_entity]),
                        BelongsToFile(file.0),
                        DefKey(def_entity),
                        QueryPlanFact(plan),
                        OperationSeed {
                            query_name: decl.name.clone(),
                            root_index,
                            root_count,
                            def_span: decl.span,
                            scope: scope.0.clone(),
                            spreads,
                        },
                    ));
                    // Self key: SQL facts carry the same key, so artifact
                    // assembly pairs each plan with its rendering.
                    commands
                        .entity(plan_entity)
                        .insert(PlanKey(plan_entity.untyped()));
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

    emit_plan_diagnostics(
        diagnostics,
        def_entity,
        catalog_entity,
        file.0,
        &mut commands,
    );
}

/// Semantic facts consumed ambiently by the per-definition plan walk,
/// bundled to stay within porridge's system-parameter arity.
#[derive(SystemParam)]
struct PlanSemanticViews<'a> {
    clauses: View<'a, (Entity, &'a ResolvedClause)>,
    aggregates: View<'a, (Entity, &'a ResolvedAggregate)>,
}

fn emit_plan_diagnostics(
    diagnostics: PlanDiagnostics,
    def_entity: Entity,
    catalog_entity: Entity,
    file: Entity,
    commands: &mut Commands<(
        dsql_schema::QueryPlan,
        dsql_schema::FragmentPlan,
        dsql_schema::Diagnostic,
    )>,
) {
    for (span, code, message) in diagnostics {
        emit_diagnostic(
            commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([def_entity, catalog_entity]),
                file,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Plan,
                code,
                message,
            },
        );
    }
}

/// Plans a fragment body against its declared table: no SQL renders from
/// it, but generated artifacts derive the fragment's result shape from
/// the plan.
#[expect(clippy::too_many_arguments, reason = "one emission site, all context")]
fn plan_fragment_body(
    planner: &Planner<'_>,
    def_entity: Entity,
    decl: &DefDecl,
    file: Entity,
    scope: &str,
    catalog_entity: Entity,
    commands: &mut Commands<(
        dsql_schema::QueryPlan,
        dsql_schema::FragmentPlan,
        dsql_schema::Diagnostic,
    )>,
    diagnostics: &mut PlanDiagnostics,
) {
    let Some((_, _, target, _)) = planner
        .tree
        .fragments
        .iter()
        .find(|(entity, _, _, _)| *entity == def_entity)
    else {
        return;
    };
    let Some(table) = planner.catalog.table_ref_for(TableRef::parse(&target.name)) else {
        // Unresolvable targets are check diagnostics; nothing to plan.
        return;
    };
    let table_id = table.id;
    // The body's spread provenance travels with the fragment plan (the
    // empty result path is the fragment root): renderers compose
    // fragment types by reuse from it.
    let mut spreads = Vec::new();
    let Some(selections) = planner.plan_selection_set(
        &mut PlanWalk {
            result_path: Vec::new(),
            spreads: &mut spreads,
            expansion: &mut SpreadExpansion::new(planner.tree, planner.scope, planner.imports),
            diagnostics,
        },
        table_id,
        SelectionPath::fragment_root(),
        &VariablePathScope::fragment(),
        def_entity,
    ) else {
        return;
    };
    commands.insert((
        DerivedFrom::many([def_entity, catalog_entity]),
        BelongsToFile(file),
        DefKey(def_entity),
        FragmentPlanFact {
            name: decl.name.clone(),
            table: table_id,
            selections,
            def_span: decl.span,
            scope: scope.to_string(),
            spreads,
        },
    ));
}

type PlanDiagnostics = Vec<(crate::facts::Span, DiagnosticCode, String)>;

/// Mutable state threaded through one plan walk. `result_path` follows
/// output keys (unchanged across spread expansion, extended per relation);
/// The shared [`SpreadExpansion`] resolves spreads and guards cycles.
struct PlanWalk<'a> {
    result_path: Vec<String>,
    spreads: &'a mut Vec<SpreadUse>,
    expansion: &'a mut SpreadExpansion<'a, 'a>,
    diagnostics: &'a mut PlanDiagnostics,
}

struct Planner<'a> {
    tree: &'a SelectionTree<'a>,
    resolved_clauses: &'a std::collections::HashMap<Entity, &'a ResolvedClause>,
    resolved_aggregates: &'a std::collections::HashMap<Entity, &'a ResolvedAggregate>,
    catalog: &'a Catalog,
    scope: &'a str,
    imports: &'a ScopeImports,
}

impl Planner<'_> {
    fn plan_collection_result(
        &self,
        walk: &mut PlanWalk<'_>,
        table: TableId,
        selection_path: SelectionPath,
        variable_scope: &VariablePathScope,
        source: Entity,
        field: &FieldSel,
    ) -> Option<CollectionResultPlan> {
        if field.has_selection_set() {
            return self
                .plan_selection_set(walk, table, selection_path, variable_scope, source)
                .map(CollectionResultPlan::Rows);
        }
        if field.has_transform() {
            return self
                .plan_aggregate(source)
                .map(CollectionResultPlan::Aggregate);
        }
        None
    }

    fn plan_aggregate(&self, source: Entity) -> Option<AggregatePlan> {
        let aggregate = self.resolved_aggregates.get(&source)?;
        if !aggregate.is_valid() {
            return None;
        }
        let group_keys = aggregate
            .group_keys
            .iter()
            .map(|key| AggregateGroupProjection {
                column: key.column,
                output_name: key.output_name.clone(),
                data_type: key.data_type,
                nullable: key.nullable,
            })
            .collect();
        let fields = aggregate
            .fields
            .iter()
            .map(|field| {
                Some(AggregateProjection {
                    function: field.function?,
                    operand: field.operand,
                    output_name: field.output_name.clone()?,
                    data_type: field.data_type?,
                    nullable: field.nullable,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(AggregatePlan {
            mode: aggregate.mode,
            group_keys,
            fields,
        })
    }

    fn plan_selection_set(
        &self,
        walk: &mut PlanWalk<'_>,
        table: TableId,
        selection_path: SelectionPath,
        variable_scope: &VariablePathScope,
        parent: Entity,
    ) -> Option<SelectionPlan> {
        let mut items = Vec::new();

        // Facts carry no sibling order between fields and spreads beyond
        // their spans, so source order is restored by merging on span order.
        enum Child<'a> {
            Field(&'a FieldSel, Entity),
            Spread(String),
        }
        let mut children: Vec<(usize, Child<'_>)> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (field.span.start, Child::Field(field, *entity)))
            .collect();
        children.extend(
            self.tree
                .spreads_under(parent)
                .map(|(_, spread, _)| (spread.span.start, Child::Spread(spread.name.clone()))),
        );
        children.sort_by_key(|(start, _)| *start);

        for (_, child) in children {
            match child {
                Child::Spread(name) => {
                    walk.spreads.push(SpreadUse {
                        path: walk.result_path.join("."),
                        fragment: name.clone(),
                    });
                    // Planning is demand-driven and runs regardless of
                    // check status, so the shared expansion's cycle cutoff
                    // guards cyclic spreads here rather than trusting
                    // checks to have rejected them.
                    let ExpandedSpread::Fragment {
                        entity: fragment_entity,
                        ..
                    } = walk.expansion.enter(&name)
                    else {
                        continue;
                    };
                    if let Some(fragment_plan) = self.plan_selection_set(
                        walk,
                        table,
                        SelectionPath::fragment_root(),
                        &variable_scope.for_fragment_spread(&selection_path, &name),
                        fragment_entity,
                    ) {
                        items.extend(fragment_plan.items);
                    }
                    walk.expansion.leave();
                }
                Child::Field(field, field_entity) => {
                    let reference = FieldRef {
                        target: TableRef::parse(&field.name),
                        selector: field.relation_path.as_deref(),
                    };
                    match self.catalog.check_field_ref(table, reference) {
                        FieldCheckResult::Column(column) => {
                            if field.body == crate::entities::field_selection::FieldBodyKind::None {
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
                            let mut child_path = selection_path.relation_child_path(
                                field.alias.clone().unwrap_or_else(|| relation_name.clone()),
                            );
                            if field.flattened && field.has_transform() {
                                child_path.push(InputPathSegment::Aggregate.as_ref().to_string());
                            }
                            let child_clauses = self.plan_clauses(
                                relation_table,
                                &child_path,
                                variable_scope,
                                field_entity,
                            );
                            if !field.flattened {
                                walk.result_path.push(
                                    field.alias.clone().unwrap_or_else(|| relation_name.clone()),
                                );
                            }
                            let nested = self.plan_collection_result(
                                walk,
                                relation_table,
                                SelectionPath::body(child_path),
                                variable_scope,
                                field_entity,
                                field,
                            );
                            if !field.flattened {
                                walk.result_path.pop();
                            }
                            if let Some(nested) = nested {
                                items.push(SelectionPlanItem::Relation(NestedRelation {
                                    relation_name: reference.display_text(),
                                    output_name: field
                                        .alias
                                        .clone()
                                        .unwrap_or(relation_name),
                                    flattened: field.flattened,
                                    foreign_key,
                                    collection: Box::new(CollectionPlan {
                                        table: relation_table,
                                        clauses: child_clauses,
                                        result: nested,
                                    }),
                                }));
                            }
                        }
                        FieldCheckResult::NotFound => {}
                        FieldCheckResult::AmbiguousRelation {
                            reference,
                            candidates,
                        } => walk.diagnostics.push((
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

        Some(SelectionPlan { items })
    }

    fn plan_clauses(
        &self,
        table: TableId,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        field_entity: Entity,
    ) -> SelectionClauses {
        let mut clauses = SelectionClauses::default();
        for (clause_entity, clause, _, _) in self.tree.clauses_under(field_entity) {
            let resolved = self.resolved_clauses.get(clause_entity).copied();
            match clause {
                ClauseFact::Where { expr } => {
                    clauses.filter = resolved.and_then(|resolved| {
                        self.plan_filter_expr(
                            table,
                            None,
                            selection_path,
                            variable_scope,
                            expr,
                            resolved,
                        )
                    });
                }
                ClauseFact::OrderBy { items } => {
                    clauses.order_by.extend(items.iter().filter_map(|item| {
                        let column_id = resolved?.order_item_at(item.field_span)?.column?;
                        let column = self.catalog.column_by_id(column_id)?;
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
                        expr,
                    );
                }
                ClauseFact::Offset { expr } => {
                    clauses.offset = plan_u64_value(
                        selection_path,
                        variable_scope,
                        VariableRole::Offset,
                        InputPathSegment::Offset,
                        expr,
                    );
                }
            }
        }
        clauses
    }

    fn plan_filter_expr(
        &self,
        table: TableId,
        outer_current_table: Option<TableId>,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        expr: &Expr,
        resolved: &ResolvedClause,
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
            Expr::Path { .. } => self.plan_filter_path(resolved, outer_current_table, expr),
            Expr::Literal { value, .. } => Some(FilterExpr::Literal(match value {
                LiteralValue::String(value) => FilterLiteral::String(value.clone()),
                LiteralValue::Number(value) => FilterLiteral::Number(value.clone()),
                LiteralValue::Bool(value) => FilterLiteral::Bool(*value),
                LiteralValue::Null => FilterLiteral::Null,
            })),
            Expr::Aggregate { .. } => {
                let aggregate = resolved.aggregate_at(expr.span())?;
                if aggregate.function != Some(crate::entities::aggregate::AggregateFunction::Exists)
                {
                    return None;
                }
                self.plan_predicate_aggregate(expr, resolved)
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let aggregate = match (lhs.as_ref(), rhs.as_ref()) {
                    (aggregate @ Expr::Aggregate { .. }, _)
                    | (_, aggregate @ Expr::Aggregate { .. }) => Some(aggregate),
                    _ => None,
                };
                if aggregate.is_some() && matches!(op, BinaryOp::Variable(_)) {
                    return None;
                }
                if let Some(aggregate) = aggregate
                    && matches!(op, BinaryOp::Comparison(_))
                    && let Some(field_path) = self.predicate_value_path(resolved, aggregate)
                {
                    let left = self.plan_aggregate_comparison_operand(
                        lhs,
                        table,
                        selection_path,
                        variable_scope,
                        &field_path,
                        op,
                        resolved,
                    )?;
                    let right = self.plan_aggregate_comparison_operand(
                        rhs,
                        table,
                        selection_path,
                        variable_scope,
                        &field_path,
                        op,
                        resolved,
                    )?;
                    return self.binary_or_variant(
                        left,
                        op,
                        right,
                        selection_path,
                        variable_scope,
                        &field_path,
                    );
                }
                if let Expr::Path { .. } = lhs.as_ref()
                    && is_comparison_operator(op)
                    && let Some(field_path) = self.predicate_path(resolved, lhs)
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
                            table,
                            Some(table),
                            selection_path,
                            variable_scope,
                            rhs,
                            resolved,
                        )?,
                    };
                    if let Some(filter) = self.relation_predicate_filter(
                        selection_path,
                        lhs,
                        op,
                        Some(field_path.join(".")),
                        right,
                        variable_scope,
                        resolved,
                    ) {
                        return Some(filter);
                    }
                }
                if let (path @ Expr::Path { .. }, Expr::Variable { variable, .. }) =
                    (lhs.as_ref(), rhs.as_ref())
                    && let Some(field_path) = self.predicate_path(resolved, path)
                {
                    let left = self.plan_filter_path(resolved, outer_current_table, path)?;
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
                    table,
                    outer_current_table,
                    selection_path,
                    variable_scope,
                    lhs,
                    resolved,
                )?;
                let (right, right_path) = self.plan_filter_expr_with_path(
                    table,
                    outer_current_table,
                    selection_path,
                    variable_scope,
                    rhs,
                    resolved,
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
        &self,
        left: FilterExpr,
        op: &BinaryOp,
        right: FilterExpr,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        inferred_path: &[String],
    ) -> Option<FilterExpr> {
        if let BinaryOp::Variable(variable) = op {
            return Some(FilterExpr::VariantBinary {
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
            });
        }
        let op = match op {
            BinaryOp::Comparison(op) => FilterOp::from(*op),
            BinaryOp::And => FilterOp::And,
            BinaryOp::Or => FilterOp::Or,
            BinaryOp::Variable(_) => return None,
        };
        Some(FilterExpr::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        })
    }

    fn plan_filter_expr_with_path(
        &self,
        table: TableId,
        outer_current_table: Option<TableId>,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        expr: &Expr,
        resolved: &ResolvedClause,
    ) -> Option<(FilterExpr, Option<String>)> {
        let field_path = self
            .predicate_value_path(resolved, expr)
            .map(|parts| parts.join("."));
        self.plan_filter_expr(
            table,
            outer_current_table,
            selection_path,
            variable_scope,
            expr,
            resolved,
        )
        .map(|expr| (expr, field_path))
    }

    fn plan_filter_path(
        &self,
        resolved_clause: &ResolvedClause,
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
            PathAnchor::Current if outer_current_table.is_some() => FilterColumnScope::OuterCurrent,
            PathAnchor::Current => FilterColumnScope::Current,
            PathAnchor::Root => FilterColumnScope::Root,
            PathAnchor::Parent => return None,
        };
        let resolved = resolved_clause.path_at(path.span())?;
        let column = resolved.terminal.column()?;
        Some(FilterExpr::Column { scope, column })
    }

    fn plan_predicate_aggregate(
        &self,
        expr: &Expr,
        resolved_clause: &ResolvedClause,
    ) -> Option<FilterExpr> {
        let aggregate = resolved_clause.aggregate_at(expr.span())?;
        if !aggregate.is_valid() {
            return None;
        }
        let relation = aggregate.relation.as_ref()?;
        Some(FilterExpr::RelationAggregate {
            foreign_key: relation.foreign_key,
            table: relation.table,
            function: aggregate.function?,
            operand: aggregate.operand,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "comparison operands share the enclosing variable and clause context"
    )]
    fn plan_aggregate_comparison_operand(
        &self,
        expr: &Expr,
        table: TableId,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        inferred_path: &[String],
        op: &BinaryOp,
        resolved: &ResolvedClause,
    ) -> Option<FilterExpr> {
        match expr {
            Expr::Aggregate { .. } => self.plan_predicate_aggregate(expr, resolved),
            Expr::Variable { variable, .. } => Some(FilterExpr::Parameter(SqlParameter {
                path: where_value_path(selection_path, variable_scope, inferred_path, op, variable),
            })),
            Expr::Binary { .. } | Expr::Literal { .. } | Expr::Path { .. } | Expr::Error { .. } => {
                self.plan_filter_expr(
                    table,
                    Some(table),
                    selection_path,
                    variable_scope,
                    expr,
                    resolved,
                )
            }
        }
    }

    /// Multi-segment `Current`-scoped predicate paths become nested
    /// `EXISTS` subqueries stepping through each relation.
    #[expect(
        clippy::too_many_arguments,
        reason = "relation planning carries its clause resolution and variable context"
    )]
    fn relation_predicate_filter(
        &self,
        selection_path: &[String],
        path: &Expr,
        op: &BinaryOp,
        operator_path: Option<String>,
        right: FilterExpr,
        variable_scope: &VariablePathScope,
        resolved_clause: &ResolvedClause,
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
        let resolved = resolved_clause.path_at(path.span())?;
        if resolved.relations.is_empty() {
            return None;
        }
        let PathTerminal::Column {
            display, column, ..
        } = &resolved.terminal
        else {
            return None;
        };
        let left = FilterExpr::Column {
            scope: FilterColumnScope::Current,
            column: *column,
        };
        let inferred =
            operator_path.map_or_else(|| vec![display.clone()], |path| path_parts(&path));
        let filter =
            self.binary_or_variant(left, op, right, selection_path, variable_scope, &inferred)?;
        Some(
            resolved
                .relations
                .iter()
                .rev()
                .fold(filter, |filter, relation| FilterExpr::Exists {
                    foreign_key: relation.foreign_key,
                    table: relation.table,
                    filter: Box::new(filter),
                }),
        )
    }

    /// The display path of a fully resolved predicate path, read from the
    /// clause resolution facts.
    fn predicate_path(&self, resolved_clause: &ResolvedClause, path: &Expr) -> Option<Vec<String>> {
        let resolved = resolved_clause.path_at(path.span())?;
        Some(resolved.display_path()?.map(str::to_owned).collect())
    }

    fn predicate_value_path(
        &self,
        resolved_clause: &ResolvedClause,
        expr: &Expr,
    ) -> Option<Vec<String>> {
        match expr {
            Expr::Path { .. } => self.predicate_path(resolved_clause, expr),
            Expr::Aggregate { .. } => resolved_clause
                .aggregate_at(expr.span())?
                .display_path(self.catalog),
            Expr::Binary { .. }
            | Expr::Literal { .. }
            | Expr::Variable { .. }
            | Expr::Error { .. } => None,
        }
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

fn path_parts(path: &str) -> Vec<String> {
    path.split('.').map(ToString::to_string).collect()
}
