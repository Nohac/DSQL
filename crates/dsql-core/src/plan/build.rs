//! The facts-to-plan builder.
//!
//! One plan per query-root selection: projections and nested relations from
//! the checked fact tree, filters lowered from expression trees (multi-step
//! relation predicates become `EXISTS` subqueries), variables becoming SQL
//! parameters with the same structured paths the variables stage infers.
//! Runs per definition, gated on [`PlanDemand`].

use crate::entities::expansion::{ExpandedSpread, SpreadExpansion};
use crate::resolution::{PathTerminal, ResolvedClause, ResolvedPath};
use crate::schema::dsql_schema;
use bowl::{Commands, DerivedFrom, Entity, Query, Registrar, SystemExt, View, With};

use super::types::{
    FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, FragmentPlanFact, NestedRelation,
    OperationSeed, OrderByPlan, Projection, QueryPlan, QueryPlanFact, SelectionClauses,
    SelectionPlan, SelectionPlanItem, SortDirectionPlan, SpreadUse, SqlParameter, SqlValue,
    SqlVariantCase,
};
use crate::catalog::{
    Catalog, CatalogSnapshot, ColumnId, FieldCheckResult, FieldRef, TableId, TableRef,
    TableResolution,
};
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
    Severity, Span, emit_diagnostic,
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
    resolutions: View<'_, (Entity, &ResolvedClause, &BelongsToFile)>,
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
    // Spans are file-unique: the definition's resolved paths and order
    // items key by span, and planning consumes resolution outcomes
    // instead of re-resolving raw strings.
    let mut resolved_paths: std::collections::HashMap<Span, &ResolvedPath> =
        std::collections::HashMap::new();
    let mut resolved_order: std::collections::HashMap<Span, ColumnId> =
        std::collections::HashMap::new();
    for (_, resolved, resolved_file) in resolutions.iter() {
        if resolved_file.0 != file.0 {
            continue;
        }
        for path in &resolved.paths {
            resolved_paths.insert(path.span, path);
        }
        for item in &resolved.order_items {
            if let Some(column) = item.column {
                resolved_order.insert(item.span, column);
            }
        }
    }
    let mut planner = Planner {
        tree: &tree,
        resolved_paths: &resolved_paths,
        resolved_order: &resolved_order,
        catalog: snapshot.catalog(),
        scope: &scope.0,
        imports,
    };
    let mut diagnostics = Vec::new();

    if decl.kind == DefKind::Fragment {
        plan_fragment_body(
            &mut planner,
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
                let selection_path = vec![response_key(field)];
                let variable_scope = VariablePathScope::operation();
                let clauses = planner.plan_clauses(
                    table_id,
                    table_id,
                    &selection_path,
                    &variable_scope,
                    root_entity,
                );
                let mut spreads = Vec::new();
                if let Some(selections) = planner.plan_selection_set(
                    &mut PlanWalk {
                        result_path: vec![output_name.clone()],
                        spreads: &mut spreads,
                        expansion: &mut SpreadExpansion::new(
                            planner.tree,
                            planner.scope,
                            planner.imports,
                        ),
                        diagnostics: &mut diagnostics,
                    },
                    table_id,
                    table_id,
                    &clauses,
                    SelectionPath::body(selection_path),
                    &variable_scope,
                    root_entity,
                ) {
                    let plan = QueryPlan {
                        root: table_id,
                        output_name,
                        clauses,
                        selections,
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
    planner: &mut Planner<'_>,
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
    // Fragment artifacts carry no spread provenance of their own; the
    // operations embedding them record it.
    let Some(selections) = planner.plan_selection_set(
        &mut PlanWalk {
            result_path: Vec::new(),
            spreads: &mut Vec::new(),
            expansion: &mut SpreadExpansion::new(planner.tree, planner.scope, planner.imports),
            diagnostics,
        },
        table_id,
        table_id,
        &SelectionClauses::default(),
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
    resolved_paths: &'a std::collections::HashMap<Span, &'a ResolvedPath>,
    resolved_order: &'a std::collections::HashMap<Span, ColumnId>,
    catalog: &'a Catalog,
    scope: &'a str,
    imports: &'a ScopeImports,
}

impl Planner<'_> {
    #[expect(
        clippy::too_many_arguments,
        reason = "recursion threads the whole walk state"
    )]
    fn plan_selection_set(
        &mut self,
        walk: &mut PlanWalk<'_>,
        root_table: TableId,
        table: TableId,
        clauses: &SelectionClauses,
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
                        root_table,
                        table,
                        &SelectionClauses::default(),
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
                                field_entity,
                            );
                            walk.result_path.push(
                                field.alias.clone().unwrap_or_else(|| relation_name.clone()),
                            );
                            let nested = self.plan_selection_set(
                                walk,
                                root_table,
                                relation_table,
                                &child_clauses,
                                SelectionPath::body(child_path),
                                variable_scope,
                                field_entity,
                            );
                            walk.result_path.pop();
                            if let Some(nested) = nested {
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
        field_entity: Entity,
    ) -> SelectionClauses {
        let mut clauses = SelectionClauses::default();
        let field_clauses: Vec<ClauseFact> = self
            .tree
            .clauses_under(field_entity)
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
                        let column_id = *self.resolved_order.get(&item.field_span)?;
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
            Expr::Path { .. } => self.plan_filter_path(outer_current_table, expr),
            Expr::Literal { value, .. } => Some(FilterExpr::Literal(match value {
                LiteralValue::String(value) => FilterLiteral::String(value.clone()),
                LiteralValue::Number(value) => FilterLiteral::Number(value.clone()),
                LiteralValue::Bool(value) => FilterLiteral::Bool(*value),
                LiteralValue::Null => FilterLiteral::Null,
            })),
            Expr::Binary { op, lhs, rhs, .. } => {
                if let Expr::Path { .. } = lhs.as_ref()
                    && is_comparison_operator(op)
                    && let Some(field_path) = self.predicate_path(lhs)
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
                    && let Some(field_path) = self.predicate_path(path)
                {
                    let left = self.plan_filter_path(outer_current_table, path)?;
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
                let field_path = self.predicate_path(expr);
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
        let resolved = self.resolved_paths.get(&path.span())?;
        let column = resolved.terminal.column()?;
        Some(FilterExpr::Column { scope, column })
    }

    /// Multi-segment `Current`-scoped predicate paths become nested
    /// `EXISTS` subqueries stepping through each relation.
    fn relation_predicate_filter(
        &mut self,
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
        let resolved = *self.resolved_paths.get(&path.span())?;
        self.relation_predicate_steps(
            resolved,
            0,
            selection_path,
            op,
            operator_path,
            right,
            variable_scope,
        )
    }

    /// Builds the nested `EXISTS` chain for one resolved relation step
    /// and recurses down the remaining steps; the innermost level compares
    /// the terminal column.
    #[expect(
        clippy::too_many_arguments,
        reason = "recursion threads the whole walk state"
    )]
    fn relation_predicate_steps(
        &mut self,
        resolved: &ResolvedPath,
        step: usize,
        selection_path: &[String],
        op: &BinaryOp,
        operator_path: Option<String>,
        right: FilterExpr,
        variable_scope: &VariablePathScope,
    ) -> Option<FilterExpr> {
        let relation = resolved.relations.get(step)?;
        let filter = if step + 1 == resolved.relations.len() {
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
            self.binary_or_variant(left, op, right, selection_path, variable_scope, &inferred)?
        } else {
            self.relation_predicate_steps(
                resolved,
                step + 1,
                selection_path,
                op,
                operator_path,
                right,
                variable_scope,
            )?
        };
        Some(FilterExpr::Exists {
            foreign_key: relation.foreign_key,
            table: relation.table,
            filter: Box::new(filter),
        })
    }

    /// The display path of a fully resolved predicate path, read from the
    /// clause resolution facts.
    fn predicate_path(&self, path: &Expr) -> Option<Vec<String>> {
        let resolved = self.resolved_paths.get(&path.span())?;
        let PathTerminal::Column { display, .. } = &resolved.terminal else {
            return None;
        };
        Some(
            resolved
                .relations
                .iter()
                .map(|step| step.display.clone())
                .chain(std::iter::once(display.clone()))
                .collect(),
        )
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

/// Output key of a selection: alias, or the object name of its target.
fn response_key(selection: &FieldSel) -> String {
    selection
        .alias
        .clone()
        .unwrap_or_else(|| TableRef::parse(&selection.name).name.to_string())
}
