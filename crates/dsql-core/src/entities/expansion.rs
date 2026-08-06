//! Fragment-spread expansion as one shared decision: which fragment a
//! spread splices in, in what order, and where cycles cut off. Checks,
//! variable inference, planning, and output-key validation all expand
//! through this walker so their behavior cannot drift — provenance stays
//! with the caller, which knows both the spread site and the fragment.

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, Registrar, Related, Where,
};

use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::field_selection::SelectionTree;
use crate::entities::fragment_spread::ResolvedSpread;
use crate::facts::{NodeKey, SemanticRoot};
use crate::schema::dsql_schema;
use crate::source::ScopeImports;

/// Stable join key shared by a semantic group and resolutions targeting it.
///
/// The key is the definition's syntax [`NodeKey`], not the group entity id, so
/// lowering can stamp it while creating the group and spread resolution can
/// copy it from the target definition. Group-side joins additionally require
/// [`SemanticRoot`] to discriminate the shared key bucket.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct SemanticDefinitionKey(pub NodeKey);

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

type SpreadDependencies<'a> = Related<
    SpreadResolutions,
    (
        &'a ResolvedSpread,
        &'a DependsOnSemanticGroup,
        &'a SemanticDefinitionKey,
    ),
>;

pub(crate) fn register_expansion(registrar: &mut Registrar<'_>) {
    registrar.system(bind_spread_dependencies);
    registrar.system(seed_expansion_occurrences);
    registrar.system(extend_expansion_occurrences);
}

/// Binds a successful spread result to exactly one target fragment group.
async fn bind_spread_dependencies(
    spreads: Query<(Entity, &ResolvedSpread, &SemanticDefinitionKey)>,
    groups: Query<
        (Entity, &SemanticRoot, &SemanticDefinitionKey),
        Where<BowlEq<SemanticDefinitionKey>>,
    >,
    mut commands: Commands<(dsql_schema::ResolvedSpread,)>,
) {
    let (spread, _, _) = spreads.item();
    let (group, _, _) = groups.item();
    commands
        .entity(spread)
        .insert(DependsOnSemanticGroup(group));
}

/// Seeds direct fragment occurrences for every query and fragment root.
async fn seed_expansion_occurrences(
    groups: Query<(Entity, &NodeKey, &SemanticRoot, SpreadDependencies<'_>)>,
    definitions: Query<(Entity, &DefDecl), Where<BowlEq<NodeKey>>>,
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

    for (resolution_entity, (spread, target_group, target_key)) in dependencies.iter() {
        let path = vec![ExpansionStep {
            resolution: resolution_entity,
            fragment: spread.name.clone(),
        }];
        if visited.iter().any(|name| name == &spread.name) {
            commands.insert((
                DerivedFrom::new(resolution_entity),
                ExpansionCycle { root_group, path },
                ExpansionCycleOf(root_group),
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
        (
            Entity,
            &SemanticRoot,
            &SemanticDefinitionKey,
            SpreadDependencies<'_>,
        ),
        Where<BowlEq<SemanticDefinitionKey>>,
    >,
    mut commands: Commands<(
        dsql_schema::ExpansionOccurrence,
        dsql_schema::ExpansionCycle,
    )>,
) {
    let (occurrence_entity, occurrence, _) = occurrences.item();
    let (_, _, _, dependencies) = targets.item();

    for (resolution_entity, (spread, target_group, target_key)) in dependencies.iter() {
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
                    path,
                },
                ExpansionCycleOf(occurrence.root_group),
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
                path,
                visited,
            },
            **target_key,
            ExpansionOccurrenceOf(occurrence.root_group),
        ));
    }
}

/// One expansion walk: resolves spread names through the effective
/// resolver and tracks the fragment path for cycle cutoff. Callers pair
/// every [`SpreadExpansion::enter`] that returned a fragment with one
/// [`SpreadExpansion::leave`].
pub(crate) struct SpreadExpansion<'t, 'v> {
    tree: &'t SelectionTree<'v>,
    scope: &'t str,
    imports: &'t ScopeImports,
    visiting: Vec<String>,
}

/// What one spread name expands to.
pub(crate) enum ExpandedSpread {
    /// The fragment is already on the expansion path: a cycle. Not
    /// entered; do not `leave`.
    Cycle,
    /// No unique visible fragment; the spread checks report why. Not
    /// entered; do not `leave`.
    Unresolved,
    /// The unique visible fragment; entered onto the path.
    Fragment { entity: Entity },
}

impl<'t, 'v> SpreadExpansion<'t, 'v> {
    pub(crate) fn new(
        tree: &'t SelectionTree<'v>,
        scope: &'t str,
        imports: &'t ScopeImports,
    ) -> Self {
        Self {
            tree,
            scope,
            imports,
            visiting: Vec::new(),
        }
    }

    /// Resolves `name` for expansion and pushes it onto the path when it
    /// yields a fragment.
    pub(crate) fn enter(&mut self, name: &str) -> ExpandedSpread {
        if self.visiting.iter().any(|visited| visited == name) {
            return ExpandedSpread::Cycle;
        }
        let Some((entity, _, _, _)) = self
            .tree
            .resolve_fragment(name, self.scope, self.imports)
            .copied()
        else {
            return ExpandedSpread::Unresolved;
        };
        self.visiting.push(name.to_string());
        ExpandedSpread::Fragment { entity }
    }

    /// Pops the most recent fragment off the expansion path.
    pub(crate) fn leave(&mut self) {
        self.visiting.pop();
    }

    /// Seeds the path with an enclosing fragment, so a body's spreads
    /// treat their own fragment as already entered (its cycle diagnostics
    /// belong to the cycle check, not to every consumer).
    pub(crate) fn seed(&mut self, name: &str) {
        self.visiting.push(name.to_string());
    }
}
