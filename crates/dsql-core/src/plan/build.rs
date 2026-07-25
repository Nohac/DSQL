//! The facts-to-plan builder.
//!
//! One plan per query definition: ordered roots containing projections and
//! nested relations from the checked fact tree, filters lowered from
//! expression trees (multi-step relation predicates become `EXISTS`
//! subqueries), variables becoming SQL parameters with the same structured
//! paths the variables stage infers. Runs per definition, gated on
//! [`PlanDemand`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::entities::expansion::{ExpandedSpread, SpreadExpansion};
use crate::resolution::{
    PathTerminal, ResolvedClause, ResolvedSelection, ResolvedSelectionShape, index_resolved_clauses,
};
use crate::schema::dsql_schema;
use bowl::{
    Commands, DerivedFrom, Entity, Eq as BowlEq, Query, Registrar, SystemExt, SystemParam, View,
    Where, With,
};

use super::types::{
    AggregateGroupProjection, AggregatePlan, AggregateProjection, CollectionPlan,
    CollectionResultPlan, ExistsKind, FilterCollection, FilterColumnScope, FilterExpr,
    FilterLiteral, FilterOp, FragmentPlanFact, NestedRelation, OperationSeed, OrderByPlan,
    PolicyAccess, PolicyApplicationField, PolicyApplicationPlan, PolicyAssignmentState,
    PolicyContextRequirement, PolicyEnforcement, PolicyFieldAccess, PolicyFieldFilter,
    PolicyFieldTarget, PolicyIdentity, Projection, QueryPlan, QueryPlanFact, QueryRootPlan,
    SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan, SpreadUse, SqlParameter,
    SqlValue, SqlVariantCase,
};
use crate::catalog::{
    Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, TableId, TableRef, TableResolution,
};
use crate::entities::aggregate::ResolvedAggregate;
use crate::entities::clause::{ClauseFact, OrderDirection, parse_pagination_value};
use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::expression::{BinaryOp, Expr, LiteralValue, PathAnchor, VariableRef};
use crate::entities::field_selection::{FieldSel, SelectionTree, TreeViews};
use crate::entities::policy::{
    CompiledPolicyField, CompiledPolicyIndex, CompiledPolicyTarget, PolicyIndex, PolicyKind,
    PolicyPlanIndex,
};
use crate::entities::variable::{
    DefinitionInputRewrites, DefinitionVariableOwner, DefinitionVariables, InputDefault,
    VariableBinding, VariableRole, VariableSource, lower_snake_case,
};
use crate::entities::variable_path::{
    InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope, VariableValue,
    predicate_anonymous_key, variable_path, variable_value,
};
use crate::facts::{
    BelongsToFile, DefKey, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    PlanDemand, PlanKey, Severity, Span, emit_diagnostic,
};
use crate::source::ScopeImports;

/// Registers the planning stage. A cross-entity stage system like
/// `lower_syntax_facts`: it walks the whole checked fact tree per definition.
pub fn register_planning(reg: &mut Registrar<'_>) {
    // Views lowered facts ambiently: behind the Complete barrier.
    reg.system(plan_queries.run_during(bowl::Phase::Complete));
    reg.system(diagnose_policy_query_context_conflicts);
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
    defs: Query<(
        Entity,
        &DefinitionVariables,
        &DefinitionInputRewrites,
        &DefinitionVariableOwner,
        &BelongsToFile,
    )>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    planning_index: Query<(
        Entity,
        &crate::entities::definition::DefIndex,
        &PolicyPlanIndex,
    )>,
    imports: Query<(Entity, &ScopeImports)>,
    views: TreeViews<'_>,
    semantic_views: PlanSemanticViews<'_>,
    mut commands: Commands<(
        dsql_schema::QueryPlan,
        dsql_schema::FragmentPlan,
        dsql_schema::Diagnostic,
    )>,
) {
    let (_, definition_variables, input_rewrites, owner, file) = defs.item();
    let def_entity = owner.definition;
    let decl = &owner.declaration;
    let scope = &owner.scope;
    let (catalog_entity, snapshot) = catalog.item();
    let (_, _, planning_index) = planning_index.item();
    let policy_index = &planning_index.resolution;
    let compiled_policies = &planning_index.compiled;
    let (_, imports) = imports.item();

    let tree = SelectionTree::collect(&views);
    let resolved_clauses =
        index_resolved_clauses(semantic_views.clauses.iter().map(|(_, resolved)| resolved));
    let resolved_aggregates = semantic_views
        .aggregates
        .iter()
        .map(|(_, aggregate)| (aggregate.source, aggregate))
        .collect();
    let resolved_selections = semantic_views
        .selections
        .iter()
        .map(|(_, selection)| (selection.field, selection))
        .collect();
    let mut planner = Planner {
        tree: &tree,
        resolved_clauses: &resolved_clauses,
        resolved_aggregates: &resolved_aggregates,
        resolved_selections: &resolved_selections,
        catalog: snapshot.catalog(),
        scope: &scope.0,
        imports,
        policy_index,
        compiled_policies,
        variables: &definition_variables.bindings,
        spread_input_rewrites: &input_rewrites.0,
        operation_assignments: Vec::new(),
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

    planner.operation_assignments =
        planner.plan_assignments(def_entity, &[], &VariablePathScope::operation(), &scope.0);

    let roots: Vec<_> = tree
        .fields_under(def_entity)
        .map(|(entity, field, _, _)| (*entity, *field))
        .collect();
    let mut planned_roots = Vec::new();
    let mut operation_spreads = Vec::new();
    let mut operation_policy_context = Vec::new();
    for (root_entity, field) in roots {
        match planner
            .catalog
            .resolve_table_ref_for(TableRef::parse(&field.name))
        {
            TableResolution::Found(table) => {
                let Some(shape) = planner.selection_shape(root_entity) else {
                    continue;
                };
                let table_id = table.id;
                let output_name = field.alias.clone().unwrap_or_else(|| table.name.clone());
                let mut selection_path = vec![field.output_key()];
                if field.flattened && field.has_transform() {
                    selection_path.push(InputPathSegment::Aggregate.as_ref().to_string());
                }
                let variable_scope = VariablePathScope::operation();
                let mut spreads = Vec::new();
                let mut policy_context = Vec::new();
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
                    policy_context: &mut policy_context,
                };
                let policies = planner.plan_source_policies(
                    table_id,
                    Some(root_entity),
                    &selection_path,
                    &variable_scope,
                    &scope.0,
                    &[],
                    walk.policy_context,
                );
                let mut policy_applications = policies.applications;
                let clauses = planner.plan_clauses(
                    table_id,
                    &selection_path,
                    &variable_scope,
                    root_entity,
                    &scope.0,
                    walk.policy_context,
                    &mut policy_applications,
                );
                if let Some(result) = planner.plan_collection_result(
                    &mut walk,
                    SelectionPath::body(selection_path),
                    &variable_scope,
                    CollectionSource {
                        table: table_id,
                        entity: root_entity,
                        field,
                    },
                    &scope.0,
                ) {
                    let policy_context = deduplicate_policy_context(
                        policy_context,
                        field.name_span,
                        &mut diagnostics,
                    );
                    operation_policy_context.extend(policy_context);
                    operation_spreads.extend(spreads);
                    planned_roots.push(QueryRootPlan {
                        output_name,
                        flattened: field.flattened,
                        collection: CollectionPlan {
                            table: table_id,
                            shape,
                            clauses,
                            policy_filter: policies.row_filter,
                            field_filters: policies.field_filters,
                            policy_nullable_fields: planner.policy_nullable_fields(table_id),
                            policy_field_access: planner.policy_field_access(table_id),
                            policy_applications,
                            result,
                        },
                    });
                }
            }
            // Root resolution failures are semantic check diagnostics; there
            // is no plan to build and the planning stage must not duplicate
            // or cascade from them.
            TableResolution::NotFound { .. } | TableResolution::Ambiguous { .. } => continue,
        }
    }

    if !planned_roots.is_empty() {
        let policy_context =
            deduplicate_policy_context(operation_policy_context, decl.span, &mut diagnostics);
        let plan_entity = commands.insert((
            DerivedFrom::many([def_entity, catalog_entity]),
            BelongsToFile(file.0),
            DefKey(def_entity),
            QueryPlanFact(QueryPlan {
                roots: planned_roots,
                policy_context,
            }),
            OperationSeed {
                query_name: decl.name.clone(),
                def_span: decl.span,
                scope: scope.0.clone(),
                spreads: operation_spreads,
            },
        ));
        // Self key: SQL facts carry the same key, so artifact assembly pairs
        // the definition plan with its rendering.
        commands
            .entity(plan_entity)
            .insert(PlanKey(plan_entity.untyped()));
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
    selections: View<'a, (Entity, &'a ResolvedSelection)>,
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

/// Rejects one trusted-context path whose query predicate and active policy
/// require incompatible value shapes. The bound [`DefKey`] join keeps the
/// diagnostic tracked on both semantic inputs, so fixing either side retires
/// it.
async fn diagnose_policy_query_context_conflicts(
    _: Query<Entity, With<DiagnosticsDemand>>,
    plans: Query<(Entity, &QueryPlanFact, &DefKey, &BelongsToFile)>,
    bindings: Query<(Entity, &VariableBinding, &DefKey, &Span), Where<BowlEq<DefKey>>>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (plan_entity, plan, _, file) = plans.item();
    let (binding_entity, binding, _, span) = bindings.item();
    if binding.source != VariableSource::Context {
        return;
    }
    let Some(requirement) = plan
        .0
        .policy_context
        .iter()
        .find(|requirement| requirement.path == binding.path)
    else {
        return;
    };
    let binding_requirement = PolicyContextRequirement {
        path: binding.path.clone(),
        data_type: binding.data_type,
        collection: binding.collection,
    };
    if !context_requirements_conflict(requirement, &binding_requirement) {
        return;
    }
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::many([plan_entity, binding_entity]),
            file: file.0,
            span: *span,
            severity: Severity::Error,
            source: DiagnosticSource::Plan,
            code: DiagnosticCode::TrustedContextTypeConflict,
            message: context_conflict_message(requirement, &binding_requirement),
        },
    );
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
    let mut policy_context = Vec::new();
    let Some(selections) = planner.plan_selection_set(
        &mut PlanWalk {
            result_path: Vec::new(),
            spreads: &mut spreads,
            expansion: &mut SpreadExpansion::new(planner.tree, planner.scope, planner.imports),
            diagnostics,
            policy_context: &mut policy_context,
        },
        table_id,
        SelectionPath::fragment_root(),
        &VariablePathScope::fragment(),
        def_entity,
        scope,
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
            policy_nullable_fields: planner.policy_nullable_fields(table_id),
            policy_field_access: planner.policy_field_access(table_id),
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
    policy_context: &'a mut Vec<PolicyContextRequirement>,
}

struct Planner<'a> {
    tree: &'a SelectionTree<'a>,
    resolved_clauses: &'a HashMap<Entity, &'a ResolvedClause>,
    resolved_aggregates: &'a HashMap<Entity, &'a ResolvedAggregate>,
    resolved_selections: &'a HashMap<Entity, &'a ResolvedSelection>,
    catalog: &'a Catalog,
    scope: &'a str,
    imports: &'a ScopeImports,
    policy_index: &'a PolicyIndex,
    compiled_policies: &'a CompiledPolicyIndex,
    variables: &'a [VariableBinding],
    spread_input_rewrites:
        &'a BTreeMap<Entity, BTreeMap<String, crate::entities::variable::SpreadInputValue>>,
    operation_assignments: Vec<PlannedPolicyAssignment>,
}

#[derive(Clone)]
struct PlannedPolicyAssignment {
    filter: Entity,
    desired: FilterExpr,
    context: Vec<PolicyContextRequirement>,
}

#[derive(Clone, Default)]
struct SourcePolicyPlan {
    row_filter: Option<FilterExpr>,
    field_filters: Vec<PolicyFieldFilter>,
    applications: Vec<PolicyApplicationPlan>,
}

struct PolicyPlanningContext<'a> {
    definition_scope: &'a str,
    context: &'a mut Vec<PolicyContextRequirement>,
    applications: &'a mut Vec<PolicyApplicationPlan>,
}

struct CollectionSource<'a> {
    table: TableId,
    entity: Entity,
    field: &'a FieldSel,
}

#[derive(Clone, Copy)]
struct FilterTableScope {
    table: TableId,
    outer_current_table: Option<TableId>,
}

impl Planner<'_> {
    fn policy_nullable_fields(&self, table: TableId) -> Vec<PolicyFieldTarget> {
        let mut fields = Vec::new();
        for field in self
            .compiled_policies
            .entries
            .iter()
            .flat_map(|entry| &entry.targets)
            .filter(|target| target.table == table)
            .flat_map(|target| &target.field_rules)
            .flat_map(|rule| &rule.fields)
            .map(|field| match field {
                CompiledPolicyField::Column(column) => PolicyFieldTarget::Column(*column),
                CompiledPolicyField::Relation(relation) => PolicyFieldTarget::Relation(*relation),
            })
        {
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
        fields
    }

    fn policy_field_access(&self, table: TableId) -> Vec<PolicyFieldAccess> {
        let mut fields = Vec::<PolicyFieldAccess>::new();
        for rule in self
            .compiled_policies
            .entries
            .iter()
            .flat_map(|entry| &entry.targets)
            .filter(|target| target.table == table)
            .flat_map(|target| &target.field_rules)
        {
            let access = PolicyAccess::for_guard(&rule.condition);
            if access == PolicyAccess::Unconditional {
                continue;
            }
            for field in &rule.fields {
                let target = match field {
                    CompiledPolicyField::Column(column) => PolicyFieldTarget::Column(*column),
                    CompiledPolicyField::Relation(relation) => {
                        PolicyFieldTarget::Relation(*relation)
                    }
                };
                if let Some(existing) = fields.iter_mut().find(|field| field.target == target) {
                    existing.access = existing.access.combine(access);
                } else {
                    fields.push(PolicyFieldAccess { target, access });
                }
            }
        }
        fields
    }

    fn selection_shape(&self, field: Entity) -> Option<ResolvedSelectionShape> {
        self.resolved_selections.get(&field)?.shape.clone()
    }

    fn fragment_scope(&self, fragment: Entity) -> Option<&str> {
        self.tree
            .fragments
            .iter()
            .find(|(entity, _, _, _)| *entity == fragment)
            .map(|(_, _, _, scope)| scope.0.as_str())
    }

    fn plan_assignments(
        &self,
        parent: Entity,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        definition_scope: &str,
    ) -> Vec<PlannedPolicyAssignment> {
        self.tree
            .clauses_under(parent)
            .filter_map(|(_, clause, _, _)| {
                let ClauseFact::FilterAssignment {
                    name, condition, ..
                } = clause
                else {
                    return None;
                };
                self.plan_assignment(
                    name,
                    condition.as_ref(),
                    selection_path,
                    variable_scope,
                    definition_scope,
                )
            })
            .collect()
    }

    fn plan_assignment(
        &self,
        name: &str,
        condition: Option<&Expr>,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        definition_scope: &str,
    ) -> Option<PlannedPolicyAssignment> {
        let candidates =
            self.policy_index
                .visible(definition_scope, PolicyKind::Filter, name, self.imports);
        let [filter] = candidates.as_slice() else {
            return None;
        };
        let mut context = Vec::new();
        let desired = condition.map_or_else(
            || Some(boolean_filter(true)),
            |condition| {
                self.plan_assignment_expr(
                    condition,
                    name,
                    selection_path,
                    variable_scope,
                    &mut context,
                )
            },
        )?;
        Some(PlannedPolicyAssignment {
            filter: filter.entity,
            desired,
            context,
        })
    }

    fn plan_assignment_expr(
        &self,
        expr: &Expr,
        filter_name: &str,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        context: &mut Vec<PolicyContextRequirement>,
    ) -> Option<FilterExpr> {
        match expr {
            Expr::Variable { variable, .. } => {
                let path = variable_path(
                    selection_path,
                    VariablePathContext {
                        role: VariableRole::FilterAssignment,
                        inferred_path: &[lower_snake_case(filter_name)],
                        anonymous_key: None,
                    },
                    variable_scope,
                    variable.sigil,
                    variable.name.as_deref(),
                );
                if variable.sigil == crate::entities::expression::Sigil::Context {
                    context.push(PolicyContextRequirement {
                        path: path.clone(),
                        data_type: crate::catalog::DataType::Boolean,
                        collection: false,
                    });
                }
                Some(FilterExpr::Parameter(SqlParameter { path }))
            }
            Expr::Literal {
                value: LiteralValue::Bool(value),
                ..
            } => Some(boolean_filter(*value)),
            Expr::Unary { operand, .. } => self
                .plan_assignment_expr(
                    operand,
                    filter_name,
                    selection_path,
                    variable_scope,
                    context,
                )
                .map(not_filter),
            Expr::Binary { op, lhs, rhs, .. } => {
                let left = self.plan_assignment_expr(
                    lhs,
                    filter_name,
                    selection_path,
                    variable_scope,
                    context,
                )?;
                let right = self.plan_assignment_expr(
                    rhs,
                    filter_name,
                    selection_path,
                    variable_scope,
                    context,
                )?;
                let op = match op {
                    BinaryOp::And => FilterOp::And,
                    BinaryOp::Or => FilterOp::Or,
                    BinaryOp::Comparison(op) => FilterOp::from(*op),
                    BinaryOp::In | BinaryOp::NotIn | BinaryOp::Variable(_) => return None,
                };
                Some(FilterExpr::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                })
            }
            Expr::NullTest {
                operand, negated, ..
            } => Some(FilterExpr::NullTest {
                operand: Box::new(self.plan_assignment_expr(
                    operand,
                    filter_name,
                    selection_path,
                    variable_scope,
                    context,
                )?),
                negated: *negated,
            }),
            Expr::List { .. }
            | Expr::Exists { .. }
            | Expr::Literal { .. }
            | Expr::Path { .. }
            | Expr::PredicateRef { .. }
            | Expr::Aggregate { .. }
            | Expr::Error { .. } => None,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "effective policy state carries source-local variable and resolution context"
    )]
    fn plan_source_policies(
        &self,
        table: TableId,
        source: Option<Entity>,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        definition_scope: &str,
        explicit: &[crate::entities::expression::FilterAssignmentExpr],
        context: &mut Vec<PolicyContextRequirement>,
    ) -> SourcePolicyPlan {
        let mut assignments = self
            .operation_assignments
            .iter()
            .cloned()
            .map(|assignment| (assignment.filter, assignment))
            .collect::<BTreeMap<_, _>>();
        if let Some(source) = source {
            for assignment in
                self.plan_assignments(source, selection_path, variable_scope, definition_scope)
            {
                assignments.insert(assignment.filter, assignment);
            }
        }
        for assignment in explicit {
            if let Some(planned) = self.plan_assignment(
                &assignment.name,
                assignment.condition.as_deref(),
                selection_path,
                variable_scope,
                definition_scope,
            ) {
                assignments.insert(planned.filter, planned);
            }
        }

        let mut identities = self
            .policy_index
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == PolicyKind::Filter
                    && entry.matches.contains(&table)
                    && self
                        .imports
                        .visible_from(self.scope)
                        .any(|scope| scope == entry.scope.as_str())
            })
            .map(|entry| entry.entity)
            .collect::<BTreeSet<_>>();
        identities.extend(assignments.keys().copied());

        let mut policies = SourcePolicyPlan::default();
        for identity in identities {
            let Some(compiled) = self.compiled_policies.entry(identity) else {
                continue;
            };
            let Some(target) = self.compiled_policies.target(identity, table) else {
                continue;
            };
            let assignment = assignments.get(&identity);
            let desired = assignment
                .map(|assignment| assignment.desired.clone())
                .unwrap_or_else(|| boolean_filter(compiled.default_active));
            let assignment_state = assignment.map_or(PolicyAssignmentState::Default, |_| {
                match filter_boolean(&desired) {
                    Some(true) => PolicyAssignmentState::Enabled,
                    Some(false) => PolicyAssignmentState::Disabled,
                    None => PolicyAssignmentState::Conditional,
                }
            });
            let enforcement = match target.enforcement.as_ref().and_then(filter_boolean) {
                Some(true) => PolicyEnforcement::Always,
                Some(false) | None if target.enforcement.is_none() => PolicyEnforcement::None,
                Some(false) => PolicyEnforcement::None,
                None => PolicyEnforcement::Conditional,
            };
            let active = target
                .enforcement
                .clone()
                .map_or(desired.clone(), |enforcement| {
                    or_filter(enforcement, desired)
                });
            let mut application = PolicyApplicationPlan {
                filter: identity,
                identity: PolicyIdentity {
                    scope: compiled.scope.clone(),
                    name: compiled.name.clone(),
                },
                conditions: compiled
                    .conditions
                    .iter()
                    .map(|condition| PolicyIdentity {
                        scope: condition.scope.clone(),
                        name: condition.name.clone(),
                    })
                    .collect(),
                path: selection_path.join("."),
                target: table,
                default_active: compiled.default_active,
                enforcement,
                assignment: assignment_state,
                rows_filtered: false,
                fields: Vec::new(),
                context: Vec::new(),
            };
            if filter_boolean(&active) == Some(false) {
                policies.applications.push(application);
                continue;
            }
            if let Some(rule) = target.row_rule.clone() {
                let guard = active_policy_guard(&active, rule);
                let mut rule_context = Vec::new();
                collect_policy_context(target, assignment, &guard, &mut rule_context);
                extend_unique_context(context, &rule_context);
                extend_unique_context(&mut application.context, &rule_context);
                application.rows_filtered =
                    PolicyAccess::for_guard(&guard) != PolicyAccess::Unconditional;
                policies.row_filter = Some(
                    policies
                        .row_filter
                        .map_or(guard.clone(), |current| and_filter(current, guard)),
                );
            }
            for rule in &target.field_rules {
                let guard = active_policy_guard(&active, rule.condition.clone());
                let access = PolicyAccess::for_guard(&guard);
                let mut field_context = Vec::new();
                collect_policy_context(target, assignment, &guard, &mut field_context);
                extend_unique_context(&mut application.context, &field_context);
                for field in &rule.fields {
                    let target = match field {
                        CompiledPolicyField::Column(column) => PolicyFieldTarget::Column(*column),
                        CompiledPolicyField::Relation(relation) => {
                            PolicyFieldTarget::Relation(*relation)
                        }
                    };
                    if access != PolicyAccess::Unconditional {
                        if let Some(existing) = application
                            .fields
                            .iter_mut()
                            .find(|field| field.target == target)
                        {
                            existing.access = existing.access.combine(access);
                        } else {
                            application
                                .fields
                                .push(PolicyApplicationField { target, access });
                        }
                    }
                    if let Some(existing) = policies
                        .field_filters
                        .iter_mut()
                        .find(|filter| filter.target == target)
                    {
                        existing.filter = and_filter(existing.filter.clone(), guard.clone());
                        for requirement in &field_context {
                            if !existing.context.contains(requirement) {
                                existing.context.push(requirement.clone());
                            }
                        }
                    } else {
                        policies.field_filters.push(PolicyFieldFilter {
                            target,
                            filter: guard.clone(),
                            context: field_context.clone(),
                        });
                    }
                }
            }
            application
                .fields
                .sort_by_key(|field| policy_field_target_sort_key(field.target));
            application
                .context
                .sort_by(|left, right| left.path.cmp(&right.path));
            policies.applications.push(application);
        }
        policies.applications.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.identity.cmp(&right.identity))
                .then_with(|| left.target.0.cmp(&right.target.0))
        });
        policies
    }

    fn plan_collection_result(
        &self,
        walk: &mut PlanWalk<'_>,
        selection_path: SelectionPath,
        variable_scope: &VariablePathScope,
        source: CollectionSource<'_>,
        definition_scope: &str,
    ) -> Option<CollectionResultPlan> {
        if source.field.has_selection_set() {
            return self
                .plan_selection_set(
                    walk,
                    source.table,
                    selection_path,
                    variable_scope,
                    source.entity,
                    definition_scope,
                )
                .map(CollectionResultPlan::Rows);
        }
        if source.field.has_transform() {
            return self
                .plan_aggregate(source.entity)
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
        definition_scope: &str,
    ) -> Option<SelectionPlan> {
        let mut items = Vec::new();

        // Facts carry no sibling order between fields and spreads beyond
        // their spans, so source order is restored by merging on span order.
        enum Child<'a> {
            Field(&'a FieldSel, Entity),
            Spread(Entity, &'a crate::entities::fragment_spread::SpreadDecl),
        }
        let mut children: Vec<(usize, Child<'_>)> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (field.span.start, Child::Field(field, *entity)))
            .collect();
        children.extend(
            self.tree
                .spreads_under(parent)
                .map(|(entity, spread, _)| (spread.span.start, Child::Spread(*entity, spread))),
        );
        children.sort_by_key(|(start, _)| *start);

        for (_, child) in children {
            match child {
                Child::Spread(spread_entity, spread) => {
                    let name = &spread.name;
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
                    } = walk.expansion.enter(name)
                    else {
                        continue;
                    };
                    if !self
                        .tree
                        .fragments
                        .iter()
                        .any(|(entity, _, _, _)| *entity == fragment_entity)
                    {
                        walk.expansion.leave();
                        continue;
                    }
                    let Some(rewrite) = self.spread_input_rewrites.get(&spread_entity) else {
                        // Contract inference omits rewrites only for unresolved or
                        // ambiguous spreads. Generation is diagnostics-gated, and
                        // planning the subtree without validated inputs would be
                        // unsound, so invalid programs fail closed here.
                        walk.expansion.leave();
                        continue;
                    };
                    let spread_scope = variable_scope.for_spread_map(rewrite.clone());
                    if let Some(fragment_plan) = self.plan_selection_set(
                        walk,
                        table,
                        SelectionPath::fragment_root(),
                        &spread_scope,
                        fragment_entity,
                        self.fragment_scope(fragment_entity)
                            .unwrap_or(definition_scope),
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
                            let Some(shape) = self.selection_shape(field_entity) else {
                                continue;
                            };
                            let relation_table = relation.table.id;
                            let relation_name = relation.name.to_string();
                            let relation_id = relation.relation.id;
                            let mut child_path = selection_path.relation_child_path(
                                field.alias.clone().unwrap_or_else(|| relation_name.clone()),
                            );
                            if field.flattened && field.has_transform() {
                                child_path.push(InputPathSegment::Aggregate.as_ref().to_string());
                            }
                            let child_policies = self.plan_source_policies(
                                relation_table,
                                Some(field_entity),
                                &child_path,
                                variable_scope,
                                definition_scope,
                                &[],
                                walk.policy_context,
                            );
                            let mut policy_applications = child_policies.applications;
                            let child_clauses = self.plan_clauses(
                                relation_table,
                                &child_path,
                                variable_scope,
                                field_entity,
                                definition_scope,
                                walk.policy_context,
                                &mut policy_applications,
                            );
                            if !field.flattened {
                                walk.result_path.push(
                                    field.alias.clone().unwrap_or_else(|| relation_name.clone()),
                                );
                            }
                            let nested = self.plan_collection_result(
                                walk,
                                SelectionPath::body(child_path),
                                variable_scope,
                                CollectionSource {
                                    table: relation_table,
                                    entity: field_entity,
                                    field,
                                },
                                definition_scope,
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
                                    relation: relation_id,
                                    collection: Box::new(CollectionPlan {
                                        table: relation_table,
                                        shape,
                                        clauses: child_clauses,
                                        policy_filter: child_policies.row_filter,
                                        field_filters: child_policies.field_filters,
                                        policy_nullable_fields: self
                                            .policy_nullable_fields(relation_table),
                                        policy_field_access: self
                                            .policy_field_access(relation_table),
                                        policy_applications,
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

    #[expect(
        clippy::too_many_arguments,
        reason = "clause planning carries both query and policy resolution context"
    )]
    fn plan_clauses(
        &self,
        table: TableId,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        field_entity: Entity,
        definition_scope: &str,
        policy_context: &mut Vec<PolicyContextRequirement>,
        policy_applications: &mut Vec<PolicyApplicationPlan>,
    ) -> SelectionClauses {
        let mut clauses = SelectionClauses::default();
        let mut invalid_pagination = false;
        let mut policy = PolicyPlanningContext {
            definition_scope,
            context: policy_context,
            applications: policy_applications,
        };
        let mut clause_rows = self
            .tree
            .clauses_under(field_entity)
            .copied()
            .collect::<Vec<_>>();
        clause_rows.sort_by_key(|(_, _, span, _)| span.start);
        for (clause_entity, clause, _, _) in clause_rows {
            let resolved = self.resolved_clauses.get(&clause_entity).copied();
            match clause {
                ClauseFact::FilterAssignment { .. } => {}
                ClauseFact::Where { expr } => {
                    clauses.filter = resolved.and_then(|resolved| {
                        self.plan_filter_expr(
                            FilterTableScope {
                                table,
                                outer_current_table: None,
                            },
                            selection_path,
                            variable_scope,
                            expr,
                            resolved,
                            &mut policy,
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
                                    match variable_value(
                                        selection_path,
                                        VariablePathContext {
                                            role: VariableRole::SortDirection,
                                            inferred_path: &[
                                                column.name.clone(),
                                                InputPathSegment::Direction.as_ref().to_string(),
                                            ],
                                            anonymous_key: None,
                                        },
                                        variable_scope,
                                        variable.sigil,
                                        variable.name.as_deref(),
                                    ) {
                                        VariableValue::Public(path) => SortDirectionPlan::Variant {
                                            nullable: self.variable_is_nullable(&path),
                                            path,
                                            variants: ["asc", "desc"]
                                                .iter()
                                                .map(|label| SqlVariantCase {
                                                    value: (*label).to_string(),
                                                    text: (*label).to_string(),
                                                })
                                                .collect(),
                                        },
                                        VariableValue::Default(InputDefault::String(value))
                                            if value == "asc" =>
                                        {
                                            SortDirectionPlan::Asc
                                        }
                                        VariableValue::Default(InputDefault::String(value))
                                            if value == "desc" =>
                                        {
                                            SortDirectionPlan::Desc
                                        }
                                        VariableValue::Default(InputDefault::Null) => return None,
                                        VariableValue::Default(_) => return None,
                                    }
                                }
                            },
                        })
                    }));
                }
                ClauseFact::Limit { expr } => {
                    match plan_pagination_value(
                        selection_path,
                        variable_scope,
                        VariableRole::Limit,
                        InputPathSegment::Limit,
                        expr,
                    ) {
                        PlannedPaginationValue::Present(value) => clauses.limit = Some(value),
                        PlannedPaginationValue::Absent => clauses.limit = None,
                        PlannedPaginationValue::Invalid => invalid_pagination = true,
                    }
                }
                ClauseFact::Offset { expr } => {
                    match plan_pagination_value(
                        selection_path,
                        variable_scope,
                        VariableRole::Offset,
                        InputPathSegment::Offset,
                        expr,
                    ) {
                        PlannedPaginationValue::Present(value) => clauses.offset = Some(value),
                        PlannedPaginationValue::Absent => clauses.offset = None,
                        PlannedPaginationValue::Invalid => invalid_pagination = true,
                    }
                }
            }
        }
        if invalid_pagination {
            clauses.limit = Some(SqlValue::Literal(0));
            clauses.offset = None;
        }
        clauses
    }

    fn plan_filter_expr(
        &self,
        table_scope: FilterTableScope,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        expr: &Expr,
        resolved: &ResolvedClause,
        policy: &mut PolicyPlanningContext<'_>,
    ) -> Option<FilterExpr> {
        match expr {
            Expr::Error { .. } => None,
            Expr::PredicateRef { .. } => None,
            Expr::Variable { variable, .. } => {
                let value = variable_value(
                    selection_path,
                    VariablePathContext {
                        role: VariableRole::WhereValue,
                        inferred_path: &[InputPathSegment::Value.as_ref().to_string()],
                        anonymous_key: None,
                    },
                    variable_scope,
                    variable.sigil,
                    variable.name.as_deref(),
                );
                let filter = self.public_filter_value(value)?;
                Some(match &filter {
                    FilterExpr::Parameter(parameter) => {
                        self.optional_filter(&parameter.path, filter.clone())
                    }
                    _ => filter,
                })
            }
            Expr::Path { .. } => {
                self.plan_filter_path(resolved, table_scope.outer_current_table, expr)
            }
            Expr::Unary { operand, .. } => self
                .plan_filter_expr(
                    table_scope,
                    selection_path,
                    variable_scope,
                    operand,
                    resolved,
                    policy,
                )
                .map(|operand| FilterExpr::Not(Box::new(operand))),
            Expr::NullTest {
                operand, negated, ..
            } => {
                let planned = self.plan_predicate_operand_path(
                    table_scope.outer_current_table,
                    operand,
                    resolved,
                )?;
                let test = FilterExpr::NullTest {
                    operand: Box::new(planned),
                    negated: *negated,
                };
                self.wrap_relation_predicate(operand, test, resolved, policy)
            }
            Expr::List { .. } => None,
            Expr::Exists {
                filters,
                predicate,
                span,
                ..
            } => {
                let existence = resolved.existence_at(*span)?;
                let source = existence.source.as_ref()?;
                let (relation, exists_table) = match source {
                    crate::resolution::ResolvedExistenceSource::Relation(relation) => {
                        (Some(relation.relation), relation.table)
                    }
                    crate::resolution::ResolvedExistenceSource::Table(table) => (None, *table),
                };
                let policies = self.plan_source_policies(
                    exists_table,
                    None,
                    selection_path,
                    variable_scope,
                    policy.definition_scope,
                    filters,
                    policy.context,
                );
                policy.applications.extend(policies.applications);
                let filter = predicate
                    .as_deref()
                    .and_then(|predicate| {
                        self.plan_filter_expr(
                            FilterTableScope {
                                table: exists_table,
                                outer_current_table: None,
                            },
                            selection_path,
                            variable_scope,
                            predicate,
                            resolved,
                            policy,
                        )
                    })
                    .map(Box::new);
                Some(FilterExpr::Exists {
                    relation,
                    table: exists_table,
                    kind: ExistsKind::Explicit,
                    source_scope: FilterColumnScope::Current,
                    policy_filter: policies.row_filter.map(Box::new),
                    field_filters: policies.field_filters,
                    filter,
                })
            }
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
                self.plan_predicate_aggregate(
                    expr,
                    resolved,
                    selection_path,
                    variable_scope,
                    policy,
                )
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                if matches!(op, BinaryOp::In | BinaryOp::NotIn) {
                    let field_path = self.predicate_path(resolved, lhs)?;
                    let operand = self.plan_predicate_operand_path(
                        table_scope.outer_current_table,
                        lhs,
                        resolved,
                    )?;
                    let collection = match rhs.as_ref() {
                        Expr::List { items, .. } => FilterCollection::List(
                            items
                                .iter()
                                .map(|item| {
                                    self.plan_filter_expr(
                                        table_scope,
                                        selection_path,
                                        variable_scope,
                                        item,
                                        resolved,
                                        policy,
                                    )
                                })
                                .collect::<Option<Vec<_>>>()?,
                        ),
                        Expr::Variable { variable, .. } => match where_variable_value(
                            selection_path,
                            variable_scope,
                            &field_path,
                            op,
                            variable,
                        ) {
                            VariableValue::Public(path) => {
                                FilterCollection::Parameter(SqlParameter { path })
                            }
                            VariableValue::Default(InputDefault::Collection(items)) => {
                                FilterCollection::List(
                                    items
                                        .iter()
                                        .map(input_default_filter)
                                        .collect::<Option<Vec<_>>>()?,
                                )
                            }
                            VariableValue::Default(InputDefault::Null) => {
                                return Some(FilterExpr::Absent);
                            }
                            VariableValue::Default(_) => return None,
                        },
                        _ => return None,
                    };
                    let parameter_path = match &collection {
                        FilterCollection::Parameter(parameter) => Some(parameter.path.clone()),
                        FilterCollection::List(_) => None,
                    };
                    let membership = FilterExpr::Membership {
                        operand: Box::new(operand),
                        collection,
                        negated: matches!(op, BinaryOp::NotIn),
                    };
                    let membership =
                        self.wrap_relation_predicate(lhs, membership, resolved, policy)?;
                    return Some(parameter_path.map_or(membership.clone(), |path| {
                        self.optional_filter(&path, membership)
                    }));
                }
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
                        table_scope.table,
                        selection_path,
                        variable_scope,
                        &field_path,
                        op,
                        resolved,
                        policy,
                    )?;
                    let right = self.plan_aggregate_comparison_operand(
                        rhs,
                        table_scope.table,
                        selection_path,
                        variable_scope,
                        &field_path,
                        op,
                        resolved,
                        policy,
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
                        Expr::Variable { variable, .. } => {
                            self.public_filter_value(where_variable_value(
                                selection_path,
                                variable_scope,
                                &field_path,
                                op,
                                variable,
                            ))?
                        }
                        _ => self.plan_filter_expr(
                            FilterTableScope {
                                table: table_scope.table,
                                outer_current_table: Some(table_scope.table),
                            },
                            selection_path,
                            variable_scope,
                            rhs,
                            resolved,
                            policy,
                        )?,
                    };
                    let optional_parameter = match &right {
                        FilterExpr::Parameter(parameter) => Some(parameter.clone()),
                        _ => None,
                    };
                    if matches!(right, FilterExpr::Absent) {
                        return Some(FilterExpr::Absent);
                    }
                    if let Some(filter) = self.relation_predicate_filter(
                        selection_path,
                        lhs,
                        op,
                        Some(field_path.join(".")),
                        right,
                        variable_scope,
                        resolved,
                        policy,
                    ) {
                        return Some(optional_parameter.map_or(filter.clone(), |parameter| {
                            self.optional_filter(&parameter.path, filter)
                        }));
                    }
                }
                if let (path @ Expr::Path { .. }, Expr::Variable { variable, .. }) =
                    (lhs.as_ref(), rhs.as_ref())
                    && let Some(field_path) = self.predicate_path(resolved, path)
                {
                    let left =
                        self.plan_filter_path(resolved, table_scope.outer_current_table, path)?;
                    let right = self.public_filter_value(where_variable_value(
                        selection_path,
                        variable_scope,
                        &field_path,
                        op,
                        variable,
                    ))?;
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
                    table_scope,
                    selection_path,
                    variable_scope,
                    lhs,
                    resolved,
                    policy,
                )?;
                let (right, right_path) = self.plan_filter_expr_with_path(
                    table_scope,
                    selection_path,
                    variable_scope,
                    rhs,
                    resolved,
                    policy,
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
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return Some(FilterExpr::Binary {
                left: Box::new(left),
                op: if matches!(op, BinaryOp::And) {
                    FilterOp::And
                } else {
                    FilterOp::Or
                },
                right: Box::new(right),
            });
        }

        if matches!(left, FilterExpr::Absent) || matches!(right, FilterExpr::Absent) {
            return Some(FilterExpr::Absent);
        }
        let (left, mut optional_parameters) = self.predicate_atom_operand(left);
        let (right, right_parameters) = self.predicate_atom_operand(right);
        for parameter in right_parameters {
            if !optional_parameters
                .iter()
                .any(|existing| existing.path == parameter.path)
            {
                optional_parameters.push(parameter);
            }
        }
        if let BinaryOp::Variable(variable) = op {
            let compares_null = matches!(
                (&left, &right),
                (FilterExpr::Literal(FilterLiteral::Null), _)
                    | (_, FilterExpr::Literal(FilterLiteral::Null))
            );
            let filter = FilterExpr::VariantBinary {
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
                variants: operator_variants(variable, compares_null),
                right: Box::new(right),
            };
            return Some(wrap_optional_atom(filter, optional_parameters));
        }
        let op = match op {
            BinaryOp::Comparison(op) => FilterOp::from(*op),
            BinaryOp::In | BinaryOp::NotIn => return None,
            BinaryOp::And | BinaryOp::Or | BinaryOp::Variable(_) => return None,
        };
        let filter = FilterExpr::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        };
        Some(wrap_optional_atom(filter, optional_parameters))
    }

    fn predicate_atom_operand(&self, operand: FilterExpr) -> (FilterExpr, Vec<SqlParameter>) {
        let mut parameters = Vec::new();
        let mut inner = operand;
        while let FilterExpr::Optional { parameter, operand } = inner {
            parameters.push(parameter);
            inner = *operand;
        }
        if !matches!(inner, FilterExpr::Parameter(_)) && !parameters.is_empty() {
            return (wrap_optional_atom(inner, parameters), Vec::new());
        }
        if let FilterExpr::Parameter(parameter) = &inner
            && self.variable_is_nullable(&parameter.path)
            && !parameters
                .iter()
                .any(|existing| existing.path == parameter.path)
        {
            parameters.push(parameter.clone());
        }
        (inner, parameters)
    }

    fn optional_filter(&self, path: &str, filter: FilterExpr) -> FilterExpr {
        if self.variable_is_nullable(path) {
            FilterExpr::Optional {
                parameter: SqlParameter {
                    path: path.to_string(),
                },
                operand: Box::new(filter),
            }
        } else {
            filter
        }
    }

    fn public_filter_value(&self, value: VariableValue) -> Option<FilterExpr> {
        match value {
            VariableValue::Public(path) => Some(FilterExpr::Parameter(SqlParameter { path })),
            VariableValue::Default(InputDefault::Null) => Some(FilterExpr::Absent),
            VariableValue::Default(default) => input_default_filter(&default),
        }
    }

    fn variable_is_nullable(&self, path: &str) -> bool {
        self.variables
            .iter()
            .find(|binding| binding.path == path)
            .is_some_and(|binding| binding.nullable)
    }

    fn plan_filter_expr_with_path(
        &self,
        table_scope: FilterTableScope,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        expr: &Expr,
        resolved: &ResolvedClause,
        policy: &mut PolicyPlanningContext<'_>,
    ) -> Option<(FilterExpr, Option<String>)> {
        let field_path = self
            .predicate_value_path(resolved, expr)
            .map(|parts| parts.join("."));
        self.plan_filter_expr(
            table_scope,
            selection_path,
            variable_scope,
            expr,
            resolved,
            policy,
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
            PathAnchor::Current if outer_current_table.is_some() => {
                FilterColumnScope::PredicateSource
            }
            PathAnchor::Current => FilterColumnScope::Current,
            PathAnchor::Root => FilterColumnScope::Root,
            PathAnchor::Parent => FilterColumnScope::Parent,
        };
        let resolved = resolved_clause.path_at(path.span())?;
        let column = resolved.terminal.column()?;
        Some(FilterExpr::Column { scope, column })
    }

    fn plan_predicate_operand_path(
        &self,
        outer_current_table: Option<TableId>,
        path: &Expr,
        resolved_clause: &ResolvedClause,
    ) -> Option<FilterExpr> {
        let Expr::Path {
            anchor, segments, ..
        } = path
        else {
            return None;
        };
        let resolved = resolved_clause.path_at(path.span())?;
        let column = resolved.terminal.column()?;
        let scope = match (anchor, segments.len()) {
            (PathAnchor::Current, 1) if outer_current_table.is_some() => {
                FilterColumnScope::PredicateSource
            }
            (PathAnchor::Current, _) => FilterColumnScope::Current,
            (PathAnchor::Root, _) => FilterColumnScope::Root,
            (PathAnchor::Parent, _) => FilterColumnScope::Parent,
        };
        Some(FilterExpr::Column { scope, column })
    }

    fn plan_predicate_aggregate(
        &self,
        expr: &Expr,
        resolved_clause: &ResolvedClause,
        selection_path: &[String],
        variable_scope: &VariablePathScope,
        policy: &mut PolicyPlanningContext<'_>,
    ) -> Option<FilterExpr> {
        let aggregate = resolved_clause.aggregate_at(expr.span())?;
        if !aggregate.is_valid() {
            return None;
        }
        let relation = aggregate.relation.as_ref()?;
        let policies = self.plan_source_policies(
            relation.table,
            None,
            selection_path,
            variable_scope,
            policy.definition_scope,
            &[],
            policy.context,
        );
        policy.applications.extend(policies.applications);
        Some(FilterExpr::RelationAggregate {
            relation: relation.relation,
            table: relation.table,
            function: aggregate.function?,
            operand: aggregate.operand,
            policy_filter: policies.row_filter.map(Box::new),
            field_filters: policies.field_filters,
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
        policy: &mut PolicyPlanningContext<'_>,
    ) -> Option<FilterExpr> {
        match expr {
            Expr::Aggregate { .. } => self.plan_predicate_aggregate(
                expr,
                resolved,
                selection_path,
                variable_scope,
                policy,
            ),
            Expr::Variable { variable, .. } => self.public_filter_value(where_variable_value(
                selection_path,
                variable_scope,
                inferred_path,
                op,
                variable,
            )),
            Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::NullTest { .. }
            | Expr::List { .. }
            | Expr::Exists { .. }
            | Expr::Literal { .. }
            | Expr::Path { .. }
            | Expr::PredicateRef { .. }
            | Expr::Error { .. } => self.plan_filter_expr(
                FilterTableScope {
                    table,
                    outer_current_table: Some(table),
                },
                selection_path,
                variable_scope,
                expr,
                resolved,
                policy,
            ),
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
        policy: &mut PolicyPlanningContext<'_>,
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
        self.wrap_relation_predicate(path, filter, resolved_clause, policy)
    }

    fn wrap_relation_predicate(
        &self,
        path: &Expr,
        filter: FilterExpr,
        resolved_clause: &ResolvedClause,
        policy: &mut PolicyPlanningContext<'_>,
    ) -> Option<FilterExpr> {
        let Expr::Path { segments, .. } = path else {
            return Some(filter);
        };
        if segments.len() < 2 {
            return Some(filter);
        }
        let resolved = resolved_clause.path_at(path.span())?;
        Some(
            resolved
                .relations
                .iter()
                .rev()
                .fold(filter, |filter, relation| {
                    let policies = self.plan_source_policies(
                        relation.table,
                        None,
                        &[],
                        &VariablePathScope::operation(),
                        policy.definition_scope,
                        &[],
                        policy.context,
                    );
                    policy.applications.extend(policies.applications);
                    FilterExpr::Exists {
                        relation: Some(relation.relation),
                        table: relation.table,
                        kind: ExistsKind::RelationshipPredicate,
                        source_scope: FilterColumnScope::Current,
                        policy_filter: policies.row_filter.map(Box::new),
                        field_filters: policies.field_filters,
                        filter: Some(Box::new(filter)),
                    }
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
            | Expr::Unary { .. }
            | Expr::NullTest { .. }
            | Expr::List { .. }
            | Expr::Exists { .. }
            | Expr::Literal { .. }
            | Expr::Variable { .. }
            | Expr::PredicateRef { .. }
            | Expr::Error { .. } => None,
        }
    }
}

/// The public path or fixed callee default of a where-value variable.
fn where_variable_value(
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    field_path: &[String],
    op: &BinaryOp,
    variable: &VariableRef,
) -> VariableValue {
    variable_value(
        selection_path,
        VariablePathContext {
            role: VariableRole::WhereValue,
            inferred_path: field_path,
            anonymous_key: variable
                .name
                .is_none()
                .then(|| predicate_anonymous_key(op))
                .flatten(),
        },
        variable_scope,
        variable.sigil,
        variable.name.as_deref(),
    )
}

fn is_comparison_operator(op: &BinaryOp) -> bool {
    matches!(op, BinaryOp::Comparison(_) | BinaryOp::Variable(_))
}

fn operator_variants(variable: &VariableRef, compares_null: bool) -> Vec<SqlVariantCase> {
    variable
        .operators
        .iter()
        .flatten()
        .filter_map(|op| {
            let op = FilterOp::from(*op);
            Some(SqlVariantCase {
                value: op.dsql_label()?.to_string(),
                text: if compares_null {
                    match op {
                        FilterOp::Eq => "is",
                        FilterOp::Ne => "is not",
                        FilterOp::Gt
                        | FilterOp::Ge
                        | FilterOp::Lt
                        | FilterOp::Le
                        | FilterOp::Like
                        | FilterOp::And
                        | FilterOp::Or => op.postgres_text()?,
                    }
                } else {
                    op.postgres_text()?
                }
                .to_string(),
            })
        })
        .collect()
}

/// A planned pagination clause, distinguishing deliberate absence from an
/// invalid value that must make forced planning fail closed.
enum PlannedPaginationValue {
    Present(SqlValue),
    Absent,
    Invalid,
}

fn plan_pagination_value(
    selection_path: &[String],
    variable_scope: &VariablePathScope,
    role: VariableRole,
    inferred_key: InputPathSegment,
    expr: &Expr,
) -> PlannedPaginationValue {
    match expr {
        Expr::Literal {
            value: LiteralValue::Number(value),
            ..
        } => parse_pagination_value(value)
            .map(|value| PlannedPaginationValue::Present(SqlValue::Literal(value)))
            .unwrap_or(PlannedPaginationValue::Invalid),
        Expr::Variable { variable, .. } => match variable_value(
            selection_path,
            VariablePathContext {
                role,
                inferred_path: &[inferred_key.as_ref().to_string()],
                anonymous_key: None,
            },
            variable_scope,
            variable.sigil,
            variable.name.as_deref(),
        ) {
            VariableValue::Public(path) => {
                PlannedPaginationValue::Present(SqlValue::Parameter(SqlParameter { path }))
            }
            VariableValue::Default(InputDefault::Number(value)) => parse_pagination_value(&value)
                .map(|value| PlannedPaginationValue::Present(SqlValue::Literal(value)))
                .unwrap_or(PlannedPaginationValue::Invalid),
            VariableValue::Default(InputDefault::Null) => PlannedPaginationValue::Absent,
            VariableValue::Default(_) => PlannedPaginationValue::Invalid,
        },
        _ => PlannedPaginationValue::Invalid,
    }
}

fn wrap_optional_atom(filter: FilterExpr, parameters: Vec<SqlParameter>) -> FilterExpr {
    parameters
        .into_iter()
        .rev()
        .fold(filter, |operand, parameter| FilterExpr::Optional {
            parameter,
            operand: Box::new(operand),
        })
}

fn input_default_filter(default: &InputDefault) -> Option<FilterExpr> {
    match default {
        InputDefault::String(value) => {
            Some(FilterExpr::Literal(FilterLiteral::String(value.clone())))
        }
        InputDefault::Number(value) => {
            Some(FilterExpr::Literal(FilterLiteral::Number(value.clone())))
        }
        InputDefault::Boolean(value) => Some(FilterExpr::Literal(FilterLiteral::Bool(*value))),
        InputDefault::Null => Some(FilterExpr::Absent),
        InputDefault::Collection(_) | InputDefault::EmptyObject => None,
    }
}

fn path_parts(path: &str) -> Vec<String> {
    path.split('.').map(ToString::to_string).collect()
}

fn boolean_filter(value: bool) -> FilterExpr {
    FilterExpr::Literal(FilterLiteral::Bool(value))
}

fn filter_boolean(filter: &FilterExpr) -> Option<bool> {
    match filter {
        FilterExpr::Literal(FilterLiteral::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn active_policy_guard(active: &FilterExpr, condition: FilterExpr) -> FilterExpr {
    if filter_boolean(active) == Some(true) {
        condition
    } else {
        or_filter(not_filter(active.clone()), condition)
    }
}

fn collect_policy_context(
    target: &CompiledPolicyTarget,
    assignment: Option<&PlannedPolicyAssignment>,
    filter: &FilterExpr,
    context: &mut Vec<PolicyContextRequirement>,
) {
    let parameter_paths = filter_parameter_paths(filter);
    context.extend(
        target
            .context
            .iter()
            .filter(|requirement| parameter_paths.contains(requirement.path.as_str()))
            .cloned(),
    );
    if let Some(assignment) = assignment {
        context.extend(
            assignment
                .context
                .iter()
                .filter(|requirement| parameter_paths.contains(requirement.path.as_str()))
                .cloned(),
        );
    }
}

fn extend_unique_context(
    context: &mut Vec<PolicyContextRequirement>,
    requirements: &[PolicyContextRequirement],
) {
    for requirement in requirements {
        if !context.contains(requirement) {
            context.push(requirement.clone());
        }
    }
}

fn policy_field_target_sort_key(target: PolicyFieldTarget) -> (u8, usize) {
    match target {
        PolicyFieldTarget::Column(column) => (0, column.0),
        PolicyFieldTarget::Relation(relation) => (1, relation.0),
    }
}

fn not_filter(filter: FilterExpr) -> FilterExpr {
    match filter {
        FilterExpr::Literal(FilterLiteral::Bool(value)) => boolean_filter(!value),
        FilterExpr::Not(operand) => *operand,
        filter => FilterExpr::Not(Box::new(filter)),
    }
}

fn or_filter(left: FilterExpr, right: FilterExpr) -> FilterExpr {
    match (filter_boolean(&left), filter_boolean(&right)) {
        (Some(true), _) | (_, Some(true)) => boolean_filter(true),
        (Some(false), _) => right,
        (_, Some(false)) => left,
        _ => FilterExpr::Binary {
            left: Box::new(left),
            op: FilterOp::Or,
            right: Box::new(right),
        },
    }
}

fn and_filter(left: FilterExpr, right: FilterExpr) -> FilterExpr {
    match (filter_boolean(&left), filter_boolean(&right)) {
        (Some(false), _) | (_, Some(false)) => boolean_filter(false),
        (Some(true), _) => right,
        (_, Some(true)) => left,
        _ => FilterExpr::Binary {
            left: Box::new(left),
            op: FilterOp::And,
            right: Box::new(right),
        },
    }
}

fn deduplicate_policy_context(
    context: Vec<PolicyContextRequirement>,
    span: Span,
    diagnostics: &mut PlanDiagnostics,
) -> Vec<PolicyContextRequirement> {
    let mut by_path: BTreeMap<String, PolicyContextRequirement> = BTreeMap::new();
    for requirement in context {
        if let Some(existing) = by_path.get(&requirement.path) {
            if context_requirements_conflict(existing, &requirement) {
                diagnostics.push((
                    span,
                    DiagnosticCode::TrustedContextTypeConflict,
                    context_conflict_message(existing, &requirement),
                ));
            }
        } else {
            by_path.insert(requirement.path.clone(), requirement);
        }
    }
    by_path.into_values().collect()
}

fn context_requirements_conflict(
    left: &PolicyContextRequirement,
    right: &PolicyContextRequirement,
) -> bool {
    left.data_type != right.data_type || left.collection != right.collection
}

fn context_conflict_message(
    left: &PolicyContextRequirement,
    right: &PolicyContextRequirement,
) -> String {
    format!(
        "trusted context `{}` is required as both {} and {}",
        left.path,
        context_requirement_shape(left),
        context_requirement_shape(right)
    )
}

fn context_requirement_shape(requirement: &PolicyContextRequirement) -> String {
    if requirement.collection {
        format!("a collection of `{}`", requirement.data_type.as_str())
    } else {
        format!("`{}`", requirement.data_type.as_str())
    }
}

fn filter_parameter_paths(filter: &FilterExpr) -> BTreeSet<&str> {
    fn collect<'a>(filter: &'a FilterExpr, paths: &mut BTreeSet<&'a str>) {
        match filter {
            FilterExpr::Optional { parameter, operand } => {
                paths.insert(&parameter.path);
                collect(operand, paths);
            }
            FilterExpr::Parameter(parameter) => {
                paths.insert(&parameter.path);
            }
            FilterExpr::Binary { left, right, .. }
            | FilterExpr::VariantBinary { left, right, .. } => {
                collect(left, paths);
                collect(right, paths);
            }
            FilterExpr::Not(operand) | FilterExpr::NullTest { operand, .. } => {
                collect(operand, paths);
            }
            FilterExpr::Membership {
                operand,
                collection,
                ..
            } => {
                collect(operand, paths);
                match collection {
                    FilterCollection::List(items) => {
                        for item in items {
                            collect(item, paths);
                        }
                    }
                    FilterCollection::Parameter(parameter) => {
                        paths.insert(&parameter.path);
                    }
                }
            }
            FilterExpr::Exists {
                policy_filter,
                field_filters,
                filter,
                ..
            } => {
                if let Some(policy_filter) = policy_filter {
                    collect(policy_filter, paths);
                }
                if let Some(filter) = filter {
                    collect(filter, paths);
                }
                for field_filter in field_filters {
                    collect(&field_filter.filter, paths);
                }
            }
            FilterExpr::RelationAggregate {
                policy_filter,
                field_filters,
                ..
            } => {
                if let Some(policy_filter) = policy_filter {
                    collect(policy_filter, paths);
                }
                for field_filter in field_filters {
                    collect(&field_filter.filter, paths);
                }
            }
            FilterExpr::Absent | FilterExpr::Column { .. } | FilterExpr::Literal(_) => {}
        }
    }

    let mut paths = BTreeSet::new();
    collect(filter, &mut paths);
    paths
}
