//! Fragment-spread expansion as one shared decision: which fragment a
//! spread splices in, in what order, and where cycles cut off. Checks,
//! variable inference, planning, and output-key validation all expand
//! through this walker so their behavior cannot drift — provenance stays
//! with the caller, which knows both the spread site and the fragment.

use std::collections::BTreeMap;

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, Registrar, Related, Where,
};

use crate::entities::aggregate::{AggregateResolutions, AggregateTransformFact, ResolvedAggregate};
use crate::entities::clause::ClauseFact;
use crate::entities::context::{ContextUseResolutions, ResolvedContextUse};
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::directive::DirectiveFact;
use crate::entities::field_selection::FieldSel;
use crate::entities::fragment_spread::{ResolvedSpread, SpreadDecl};
use crate::entities::variable::VariableUse;
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    SemanticMembers, SemanticRoot, Severity, Span, emit_diagnostic,
};
use crate::resolution::{
    ClauseResolutions, ResolvedClause, ResolvedSelection, SelectionResolutions,
};
use crate::schema::dsql_schema;
use crate::source::ResolutionScope;

/// Stable join key shared by a definition, its semantic group, and resolutions
/// targeting that group.
///
/// The key is the definition entity, whose reconciled identity survives
/// syntax-only changes that move CST node indices. Bound queries must also
/// require their domain's tenant component: groups use [`SemanticRoot`],
/// syntax definitions use [`DefDecl`], and derived contracts use their own
/// specific payload component. [`SemanticRoot`] also contains the definition
/// entity, but remains distinct because it is a group-side discriminator
/// rather than a join key.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct SemanticDefinitionKey(pub Entity);

/// Relationship edge from one [`ResolvedSpread`] to its source group.
/// Selection, clause, and aggregate resolutions use separate relationships so
/// their changes cannot invalidate fragment expansion.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = SpreadResolutions)]
pub struct SpreadResolutionOf(pub Entity);

/// Engine-maintained resolved spreads derived from one semantic group.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = SpreadResolutionOf)]
pub struct SpreadResolutions(pub Vec<Entity>);

/// Relationship edge from a resolved spread to the fragment group it uses.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
#[relationship(target = SemanticDependents)]
pub struct DependsOnSemanticGroup(pub Entity);

/// Engine-maintained spread resolutions that depend on one fragment group.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = DependsOnSemanticGroup)]
pub struct SemanticDependents(pub Vec<Entity>);

/// One spread step in an expansion path.
///
/// The resolution entity is an internal invocation key. Diagnostics and
/// snapshots project `fragment` instead because derived entity ids are not
/// stable between cold and incremental bowls.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExpansionStep {
    pub resolution: Entity,
    pub fragment: String,
}

/// One occurrence of a fragment body along one simple spread path.
///
/// `root_group`, `target_group`, and each step's resolution are internal
/// identity. Cold/incremental comparisons and user-facing provenance must
/// project semantic names and spans rather than comparing this component raw.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ExpansionOccurrence {
    pub root_group: Entity,
    pub target_group: Entity,
    /// Parent occurrence, or `None` when the incoming spread is in the root.
    pub parent: Option<Entity>,
    /// Source spread syntax entity that introduced this occurrence.
    pub incoming_spread: Entity,
    /// Kind of the semantic root expanding this occurrence.
    pub root_kind: DefKind,
    pub path: Vec<ExpansionStep>,
    visited: Vec<String>,
}

/// Relationship edge from an occurrence to the semantic root expanding it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ExpansionOccurrences)]
pub struct ExpansionOccurrenceOf(pub Entity);

/// Engine-maintained expansion occurrences for one semantic root.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ExpansionOccurrenceOf)]
pub struct ExpansionOccurrences(pub Vec<Entity>);

/// A spread edge cut because its fragment name already occurs on the path.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ExpansionCycle {
    pub root_group: Entity,
    pub root_kind: DefKind,
    pub parent: Option<Entity>,
    pub closing_spread: Entity,
    pub file: Entity,
    pub name: String,
    pub name_span: Span,
    pub path: Vec<ExpansionStep>,
}

/// Relationship edge from a cycle to the semantic root expanding it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ExpansionCycles)]
pub struct ExpansionCycleOf(pub Entity);

/// Engine-maintained expansion cycles for one semantic root.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ExpansionCycleOf)]
pub struct ExpansionCycles(pub Vec<Entity>);

/// Dedicated relationship owner for one spread's semantic products.
///
/// This group keeps candidate and cycle inverses off both syntax and
/// semantic-root entities, whose revisions anchor unrelated derived facts.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct SpreadSiteRoot;

/// Untracked pointer copied onto a resolved spread for expansion and checks.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
pub struct SpreadSiteGroup(pub Entity);

/// Relationship edge from a cycle candidate to its dedicated closing site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ClosingSpreadCycles)]
pub struct ExpansionCycleAt(pub Entity);

/// Engine-maintained cycle candidates that close at one spread site.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ExpansionCycleAt)]
pub struct ClosingSpreadCycles(pub Vec<Entity>);

type SpreadDependencies<'a> = Related<
    SpreadResolutions,
    (
        &'a ResolvedSpread,
        &'a DependsOnSemanticGroup,
        &'a SemanticDefinitionKey,
        &'a BelongsToFile,
        &'a SpreadSiteGroup,
    ),
>;

/// One lowered semantic member copied into an expansion occurrence.
#[derive(Debug, Clone, Hash)]
pub struct ExpansionMember {
    pub source: Entity,
    pub parent: Option<Entity>,
    pub span: Option<Span>,
    pub field: Option<FieldSel>,
    pub spread: Option<SpreadDecl>,
    pub clause: Option<ClauseFact>,
    pub aggregate: Option<AggregateTransformFact>,
    pub directive: Option<DirectiveFact>,
    pub variable: Option<VariableUse>,
}

/// The complete semantic body of one expanded fragment occurrence.
///
/// Source entity ids are qualified by the owning occurrence when consumers
/// build a closure; the same fragment syntax can therefore appear repeatedly
/// without collapsing distinct call paths.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub struct ExpansionBody {
    pub occurrence: Entity,
    pub parent: Option<Entity>,
    pub incoming_spread: Entity,
    pub target_group: Entity,
    pub definition: Entity,
    pub declaration: DefDecl,
    pub target: Option<FragmentTarget>,
    pub scope: ResolutionScope,
    pub file: Entity,
    pub members: Vec<ExpansionMember>,
    pub selections: Vec<ResolvedSelection>,
    pub clauses: Vec<ResolvedClause>,
    pub context_uses: Vec<ResolvedContextUse>,
    pub aggregates: Vec<ResolvedAggregate>,
    pub spreads: Vec<ResolvedSpread>,
}

/// Relationship edge from a materialized body to the semantic root using it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ExpansionBodies)]
pub struct ExpansionBodyOf(pub Entity);

/// Engine-maintained expansion bodies used by one semantic root.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ExpansionBodyOf)]
pub struct ExpansionBodies(pub Vec<Entity>);

pub(crate) type RawSemanticMembers<'a> = Related<
    // Relationship query tuples currently support at most eight parts. Split
    // this raw payload before adding another semantic member component.
    SemanticMembers,
    (
        Option<&'a ChildOf>,
        Option<&'a FieldSel>,
        Option<&'a SpreadDecl>,
        Option<&'a ClauseFact>,
        Option<&'a AggregateTransformFact>,
        Option<&'a DirectiveFact>,
        Option<&'a VariableUse>,
        Option<&'a Span>,
    ),
>;

pub(crate) type SelectionResolutionRows<'a> =
    Related<SelectionResolutions, (&'a ResolvedSelection,)>;
pub(crate) type ClauseResolutionRows<'a> = Related<ClauseResolutions, (&'a ResolvedClause,)>;
pub(crate) type ContextUseResolutionRows<'a> =
    Related<ContextUseResolutions, (&'a ResolvedContextUse,)>;
pub(crate) type AggregateResolutionRows<'a> =
    Related<AggregateResolutions, (&'a ResolvedAggregate,)>;
pub(crate) type SpreadResolutionRows<'a> = Related<SpreadResolutions, (&'a ResolvedSpread,)>;

type ExpansionTargetGroups<'a> = Query<
    (
        Entity,
        &'a SemanticRoot,
        RawSemanticMembers<'a>,
        SelectionResolutionRows<'a>,
        ClauseResolutionRows<'a>,
        ContextUseResolutionRows<'a>,
    ),
    Where<BowlEq<SemanticDefinitionKey>>,
>;
type ExpansionResolutionGroups<'a> = Query<
    (
        Entity,
        &'a SemanticRoot,
        AggregateResolutionRows<'a>,
        SpreadResolutionRows<'a>,
    ),
    Where<BowlEq<SemanticDefinitionKey>>,
>;
type ExpansionDefinitions<'a> = Query<
    (
        Entity,
        &'a DefDecl,
        Option<&'a FragmentTarget>,
        &'a ResolutionScope,
        &'a BelongsToFile,
    ),
    Where<BowlEq<SemanticDefinitionKey>>,
>;

pub(crate) fn clone_semantic_members(members: &RawSemanticMembers<'_>) -> Vec<ExpansionMember> {
    members
        .iter()
        .map(
            |(source, (parent, field, spread, clause, aggregate, directive, variable, span))| {
                ExpansionMember {
                    source,
                    parent: parent.map(|parent| parent.0),
                    span: span.copied(),
                    field: field.cloned(),
                    spread: spread.cloned(),
                    clause: clause.cloned(),
                    aggregate: aggregate.cloned(),
                    directive: directive.cloned(),
                    variable: variable.cloned(),
                }
            },
        )
        .collect()
}

pub(crate) fn clone_selection_resolutions(
    resolutions: &SelectionResolutionRows<'_>,
) -> Vec<ResolvedSelection> {
    resolutions
        .iter()
        .map(|(_, (resolved,))| (*resolved).clone())
        .collect()
}

pub(crate) fn clone_clause_resolutions(
    resolutions: &ClauseResolutionRows<'_>,
) -> Vec<ResolvedClause> {
    resolutions
        .iter()
        .map(|(_, (resolved,))| (*resolved).clone())
        .collect()
}

pub(crate) fn clone_aggregate_resolutions(
    resolutions: &AggregateResolutionRows<'_>,
) -> Vec<ResolvedAggregate> {
    resolutions
        .iter()
        .map(|(_, (resolved,))| (*resolved).clone())
        .collect()
}

pub(crate) fn clone_spread_resolutions(
    resolutions: &SpreadResolutionRows<'_>,
) -> Vec<ResolvedSpread> {
    resolutions
        .iter()
        .map(|(_, (resolved,))| (*resolved).clone())
        .collect()
}

pub(crate) fn clone_context_use_resolutions(
    resolutions: &ContextUseResolutionRows<'_>,
) -> Vec<ResolvedContextUse> {
    resolutions
        .iter()
        .map(|(_, (resolved,))| (*resolved).clone())
        .collect()
}

pub(crate) fn register_expansion(registrar: &mut Registrar<'_>) {
    registrar.system(bind_spread_dependencies);
    registrar.system(seed_expansion_occurrences);
    registrar.system(extend_expansion_occurrences);
    registrar.system(materialize_expansion_bodies);
    registrar.system(check_expansion_cycles);
}

/// Binds a successful spread result to exactly one target fragment group.
async fn bind_spread_dependencies(
    spreads: Query<(Entity, &ResolvedSpread, &SemanticDefinitionKey)>,
    groups: Query<(Entity, &SemanticRoot), Where<BowlEq<SemanticDefinitionKey>>>,
    mut commands: Commands<(dsql_schema::ResolvedSpread,)>,
) {
    let (spread, _, _) = spreads.item();
    let (group, _) = groups.item();
    commands
        .entity(spread)
        .insert(DependsOnSemanticGroup(group));
}

/// Seeds direct fragment occurrences for every query and fragment root.
async fn seed_expansion_occurrences(
    groups: Query<(
        Entity,
        &SemanticDefinitionKey,
        &SemanticRoot,
        SpreadDependencies<'_>,
    )>,
    definitions: Query<(Entity, &DefDecl), Where<BowlEq<SemanticDefinitionKey>>>,
    mut commands: Commands<(
        dsql_schema::ExpansionOccurrence,
        dsql_schema::ExpansionCycle,
    )>,
) {
    let (root_group, _, _, dependencies) = groups.item();
    let (_, definition) = definitions.item();
    let visited = if definition.kind == DefKind::Fragment {
        vec![definition.name.clone()]
    } else {
        Vec::new()
    };

    for (resolution_entity, (spread, target_group, target_key, file, cycle_site)) in
        dependencies.iter()
    {
        let path = vec![ExpansionStep {
            resolution: resolution_entity,
            fragment: spread.name.clone(),
        }];
        if visited.iter().any(|name| name == &spread.name) {
            commands.insert((
                DerivedFrom::new(resolution_entity),
                ExpansionCycle {
                    root_group,
                    root_kind: definition.kind,
                    parent: None,
                    closing_spread: spread.spread,
                    file: file.0,
                    name: spread.name.clone(),
                    name_span: spread.name_span,
                    path,
                },
                ExpansionCycleOf(root_group),
                ExpansionCycleAt(cycle_site.0),
            ));
            continue;
        }

        let mut next_visited = visited.clone();
        next_visited.push(spread.name.clone());
        commands.insert((
            DerivedFrom::new(resolution_entity),
            ExpansionOccurrence {
                root_group,
                target_group: target_group.0,
                parent: None,
                incoming_spread: spread.spread,
                root_kind: definition.kind,
                path,
                visited: next_visited,
            },
            **target_key,
            ExpansionOccurrenceOf(root_group),
        ));
    }
}

/// Extends every occurrence through the spreads in its target fragment.
///
/// This system intentionally reads and writes [`ExpansionOccurrence`]. Each
/// invocation owns only its direct children, so convergence advances one
/// fragment depth at a time and retiring a parent retires its entire derived
/// subtree.
async fn extend_expansion_occurrences(
    occurrences: Query<(Entity, &ExpansionOccurrence, &SemanticDefinitionKey)>,
    targets: Query<
        (Entity, &SemanticRoot, SpreadDependencies<'_>),
        Where<BowlEq<SemanticDefinitionKey>>,
    >,
    mut commands: Commands<(
        dsql_schema::ExpansionOccurrence,
        dsql_schema::ExpansionCycle,
    )>,
) {
    let (occurrence_entity, occurrence, _) = occurrences.item();
    let (_, _, dependencies) = targets.item();

    for (resolution_entity, (spread, target_group, target_key, file, cycle_site)) in
        dependencies.iter()
    {
        let mut path = occurrence.path.clone();
        path.push(ExpansionStep {
            resolution: resolution_entity,
            fragment: spread.name.clone(),
        });
        if occurrence.visited.iter().any(|name| name == &spread.name) {
            commands.insert((
                DerivedFrom::many([occurrence_entity, resolution_entity]),
                ExpansionCycle {
                    root_group: occurrence.root_group,
                    root_kind: occurrence.root_kind,
                    parent: Some(occurrence_entity),
                    closing_spread: spread.spread,
                    file: file.0,
                    name: spread.name.clone(),
                    name_span: spread.name_span,
                    path,
                },
                ExpansionCycleOf(occurrence.root_group),
                ExpansionCycleAt(cycle_site.0),
            ));
            continue;
        }

        let mut visited = occurrence.visited.clone();
        visited.push(spread.name.clone());
        commands.insert((
            DerivedFrom::many([occurrence_entity, resolution_entity]),
            ExpansionOccurrence {
                root_group: occurrence.root_group,
                target_group: target_group.0,
                parent: Some(occurrence_entity),
                incoming_spread: spread.spread,
                root_kind: occurrence.root_kind,
                path,
                visited,
            },
            **target_key,
            ExpansionOccurrenceOf(occurrence.root_group),
        ));
    }
}

/// Copies one target fragment's exact semantic rows into each occurrence.
/// The relationship projections are scoped to the target group, so edits to
/// unrelated definitions cannot wake this invocation.
async fn materialize_expansion_bodies(
    occurrences: Query<(Entity, &ExpansionOccurrence, &SemanticDefinitionKey)>,
    groups: ExpansionTargetGroups<'_>,
    resolution_groups: ExpansionResolutionGroups<'_>,
    definitions: ExpansionDefinitions<'_>,
    mut commands: Commands<(dsql_schema::ExpansionBody,)>,
) {
    let (occurrence_entity, occurrence, _) = occurrences.item();
    let (target_group, _, members, selections, clauses, context_uses) = groups.item();
    let (_, _, aggregates, spreads) = resolution_groups.item();
    let (definition, declaration, target, scope, file) = definitions.item();

    let members = clone_semantic_members(&members);
    let selections = clone_selection_resolutions(&selections);
    let clauses = clone_clause_resolutions(&clauses);
    let context_uses = clone_context_use_resolutions(&context_uses);
    let aggregates = clone_aggregate_resolutions(&aggregates);
    let spreads = clone_spread_resolutions(&spreads);

    commands.insert((
        DerivedFrom::new(occurrence_entity),
        ExpansionBody {
            occurrence: occurrence_entity,
            parent: occurrence.parent,
            incoming_spread: occurrence.incoming_spread,
            target_group,
            definition,
            declaration: declaration.clone(),
            target: target.cloned(),
            scope: scope.clone(),
            file: file.0,
            members,
            selections,
            clauses,
            context_uses,
            aggregates,
            spreads,
        },
        ExpansionBodyOf(occurrence.root_group),
    ));
}

type ClosingSpreadCycleRows<'a> = Related<ClosingSpreadCycles, (&'a ExpansionCycle,)>;

/// Reports each intrinsic fragment cycle once per closing spread site.
///
/// Multiple expansion paths and fragment roots can reach the same closing
/// spread. Its dedicated site group selects one semantic representative
/// without waking on unrelated members of any root.
async fn check_expansion_cycles(
    _: Query<Entity, bowl::With<DiagnosticsDemand>>,
    sites: Query<(Entity, &SpreadSiteRoot, ClosingSpreadCycleRows<'_>)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (_, _, cycles) = sites.item();
    let mut unique = BTreeMap::<Entity, ExpansionCycle>::new();
    for (_, (cycle,)) in cycles.iter() {
        if cycle.root_kind != DefKind::Fragment {
            continue;
        }
        let candidate_key = cycle_semantic_key(cycle);
        let replace = unique
            .get(&cycle.closing_spread)
            .is_none_or(|current| candidate_key < cycle_semantic_key(current));
        if replace {
            unique.insert(cycle.closing_spread, (*cycle).clone());
        }
    }

    for cycle in unique.into_values() {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(cycle.closing_spread),
                file: cycle.file,
                span: cycle.name_span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::CircularFragmentSpread,
                message: format!("fragment `{}` recursively spreads itself", cycle.name),
            },
        );
    }
}

fn cycle_semantic_key(cycle: &ExpansionCycle) -> (usize, Vec<&str>) {
    (
        cycle.path.len(),
        cycle
            .path
            .iter()
            .map(|step| step.fragment.as_str())
            .collect(),
    )
}
