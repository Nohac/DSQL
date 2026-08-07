//! Definition entity: named top-level definitions (queries and fragments),
//! the definition index, and the duplicate-fragment check.
//!
//! Queries and fragments are one entity because they are structurally the
//! same concept — a named definition with a selection set — and every stage
//! treats them symmetrically except where [`DefKind`] branches.

use crate::schema::{AstFacts, dsql_schema};
use std::{collections::BTreeMap, fmt};

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, Registrar, Related, Where, With,
};

use crate::entities::variable::{
    DefinitionVariables, InputRefinement, VariableBinding, VariableRole, build_input_refinements,
    input_default_label, variable_type_label,
};
use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{NodeRef, Rule};
use crate::plan::{
    DynamicInputContract, DynamicInputKind, DynamicPredicateOperator, QueryPlanFact,
};
use crate::resolution::{ResolvedFragmentTarget, ResolvedTableTarget};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::source::{ResolutionScope, ScopeImports};

/// What kind of definition a [`DefDecl`] fact describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DefKind {
    Query,
    Fragment,
}

impl fmt::Display for DefKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefKind::Query => f.write_str("query"),
            DefKind::Fragment => f.write_str("fragment"),
        }
    }
}

/// One named top-level definition, lowered from `query_def`/`fragment_def`.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub struct DefDecl {
    pub kind: DefKind,
    pub name: String,
    /// Span of the name token, for name-precision diagnostics.
    pub name_span: Span,
    /// Span of the whole definition.
    pub span: Span,
    /// Fingerprint of the definition's source slice, retained for the
    /// remaining definition-level consumers until every body contract is a
    /// relationship-owned semantic fact.
    pub source_hash: u64,
    /// Contract refinements written in the definition header.
    pub input_refinements: Vec<InputRefinement>,
}

/// The relation a fragment is declared `on`. Only fragment entities carry
/// this; the catalog check (phase 6) validates it against the schema.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub struct FragmentTarget {
    pub name: String,
    pub span: Span,
}

/// Join key carried by fragment definitions and fragment spreads alike, so
/// spread resolution is a bound join on the name (see `fragment_spread`).
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct FragmentKey(pub String);

/// Exact semantic name shared by definitions of one kind.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionNameKey {
    pub kind: DefKind,
    pub name: String,
}

/// Source path carried only for navigation and deterministic diagnostics.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionPath(pub String);

/// Stable key shared by one definition and its dedicated candidate site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionSiteKey(pub Entity);

/// Span-independent definition identity used to bind visibility candidates.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionSemantics {
    pub definition: Entity,
    pub provider_scope: String,
}

/// Source target for diagnostics that must follow definition movement.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionNavigation {
    pub definition: Entity,
    pub file: Entity,
    pub name_span: Span,
    pub provider_scope: String,
    pub file_path: String,
}

/// Dedicated owner for one definition's visible same-name candidates.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionSiteRoot;

/// Consumer payload colocated with a definition's dedicated candidate site.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefinitionSiteContext {
    pub definition: Entity,
    pub provider_scope: String,
    pub name_key: DefinitionNameKey,
    pub file: Entity,
    pub name_span: Span,
    pub file_path: String,
}

/// Relationship edge from diagnostic context to its stable definition site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = DefinitionSiteContexts)]
pub struct DefinitionSiteContextOf(pub Entity);

/// Engine-maintained diagnostic context for one definition site.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = DefinitionSiteContextOf)]
pub struct DefinitionSiteContexts(pub Vec<Entity>);

/// A same-name definition visible from one definition site.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct VisibleDefinitionCandidate {
    pub provider: Entity,
    pub provider_scope: String,
}

/// Relationship edge from a visible definition to its consumer site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = VisibleDefinitionCandidates)]
pub struct VisibleDefinitionCandidateOf(pub Entity);

/// Engine-maintained same-name definitions visible from one definition.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = VisibleDefinitionCandidateOf)]
pub struct VisibleDefinitionCandidates(pub Vec<Entity>);

/// A same-name query peer, annotated with every consumer scope that imports
/// both definitions.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ImportedQueryPeer {
    pub peer: Entity,
    pub peer_scope: String,
    pub peer_path: String,
    pub peer_name_span: Span,
    pub consumer_scopes: Vec<String>,
}

/// Relationship edge from an imported query peer to one definition site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ImportedQueryPeers)]
pub struct ImportedQueryPeerOf(pub Entity);

/// Engine-maintained same-name peers for one query definition.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ImportedQueryPeerOf)]
pub struct ImportedQueryPeers(pub Vec<Entity>);

/// Owns `query_def` and `fragment_def`.
pub struct Definition;

impl LanguageEntity for Definition {
    const NAME: &'static str = "definition";

    fn register(reg: &mut Registrar<'_>) {
        reg.system(project_definition_sites);
        reg.system(project_definition_facts);
        reg.system(enrich_definition_sites);
        reg.system(bind_visible_definition_candidates);
        reg.system(check_definition_conflicts);
        reg.system(bind_imported_query_peers);
        reg.system(check_import_ambiguities);
        reg.system(check_fragment_targets);
        // Fully tracked (per-file and per-definition bound joins, no views),
        // so it needs no phase barrier: replanning orders it after enrichment,
        // variable inference, and the optional plan capability contract.
        reg.system(hover_definitions);
    }
}

/// A fragment's `on` target must resolve to a catalog table; its body is
/// only checked once it does (see the field-selection check systems).
async fn check_fragment_targets(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &ResolvedFragmentTarget, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (resolution, target, file) = query.item();

    match &target.target {
        ResolvedTableTarget::Table(_) => {}
        ResolvedTableTarget::NotFound { reference } => {
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::new(resolution),
                    file: file.0,
                    span: target.span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::TableNotFound,
                    message: format!("table `{reference}` not found"),
                },
            );
        }
        ResolvedTableTarget::Ambiguous {
            reference,
            candidates,
        } => {
            let candidates: Vec<String> = candidates
                .iter()
                .map(|key| format!("{}::{}", key.schema, key.table))
                .collect();
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::new(resolution),
                    file: file.0,
                    span: target.span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::AmbiguousTable,
                    message: format!(
                        "table `{reference}` is ambiguous; use an alias with a schema-qualified name ({})",
                        candidates.join(", ")
                    ),
                },
            );
        }
    }
}

impl LowerStage for Definition {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        // The name is a direct child token; nested Names (inside the
        // selection set) belong to other entities.
        let Some(name_span) = direct_name(ctx.cst, node) else {
            // Error recovery can leave a def without a name; the parse
            // diagnostics already cover it.
            return None;
        };

        let kind = if ctx.cst.match_rule(node, Rule::QueryDef) {
            DefKind::Query
        } else {
            DefKind::Fragment
        };

        let span = node_span(ctx.cst, node);
        let source_hash = crate::source::content_hash(text(ctx.source, span));
        let header_rule = match kind {
            DefKind::Query => Rule::QueryHeader,
            DefKind::Fragment => Rule::FragmentHeader,
        };
        let input_refinements = direct_rule(ctx.cst, node, header_rule)
            .map(|header| build_input_refinements(ctx.cst, ctx.source, header))
            .unwrap_or_default();
        let decl = DefDecl {
            kind,
            name: text(ctx.source, name_span).to_string(),
            name_span,
            span,
            source_hash,
            input_refinements,
        };
        let definition_name_key = DefinitionNameKey {
            kind,
            name: decl.name.clone(),
        };

        let target = direct_rule(ctx.cst, node, Rule::QualifiedName).map(|target| {
            let span = node_span(ctx.cst, target);
            FragmentTarget {
                name: text(ctx.source, span).to_string(),
                span,
            }
        });

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let scope = ResolutionScope(ctx.scope.to_string());
        let entity = match (kind, target) {
            (DefKind::Fragment, Some(target)) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                scope,
                definition_name_key,
                DefinitionPath(ctx.path.to_string()),
                FragmentKey(decl.name.clone()),
                decl,
                target,
            )),
            // A fragment whose `on` target was lost to error recovery still
            // lowers (spreads may resolve to it); the parse diagnostics
            // already report the malformed target.
            (DefKind::Fragment, None) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                scope,
                definition_name_key,
                DefinitionPath(ctx.path.to_string()),
                FragmentKey(decl.name.clone()),
                decl,
            )),
            (DefKind::Query, _) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                scope,
                definition_name_key,
                DefinitionPath(ctx.path.to_string()),
                decl,
            )),
        };
        Some(entity.untyped())
    }
}

type DefinitionCandidateRows<'a> =
    Related<VisibleDefinitionCandidates, (&'a VisibleDefinitionCandidate,)>;
type DefinitionSiteContextRows<'a> = Related<DefinitionSiteContexts, (&'a DefinitionSiteContext,)>;
type ImportedQueryRows<'a> = Related<ImportedQueryPeers, (&'a ImportedQueryPeer,)>;

/// Projects one definition into separate semantic and navigation products.
/// The definition drives both outputs, so a changed declaration reaches its
/// exact products in one hint hop.
async fn project_definition_facts(
    definitions: Query<(
        Entity,
        &DefDecl,
        &DefinitionNameKey,
        &ResolutionScope,
        &BelongsToFile,
        &DefinitionPath,
        &crate::entities::expansion::SemanticDefinitionKey,
    )>,
    mut commands: Commands<(
        dsql_schema::DefinitionSemanticProjection,
        dsql_schema::DefinitionNavigationProjection,
    )>,
) {
    let (definition, declaration, name_key, scope, file, path, semantic_key) = definitions.item();
    let site_key = DefinitionSiteKey(definition);
    commands.insert((
        DerivedFrom::new(definition),
        BelongsToFile(file.0),
        name_key.clone(),
        *semantic_key,
        site_key,
        DefinitionSemantics {
            definition,
            provider_scope: scope.0.clone(),
        },
    ));
    commands.insert((
        DerivedFrom::new(definition),
        name_key.clone(),
        *semantic_key,
        site_key,
        DefinitionNavigation {
            definition,
            file: file.0,
            name_span: declaration.name_span,
            provider_scope: scope.0.clone(),
            file_path: path.0.clone(),
        },
    ));
}

/// Creates one stable relationship owner from the definition's immutable
/// semantic identity. Definition body, name, and span changes do not recreate
/// this entity.
async fn project_definition_sites(
    definitions: Query<(
        Entity,
        &DefDecl,
        &crate::entities::expansion::SemanticDefinitionKey,
    )>,
    mut commands: Commands<(dsql_schema::DefinitionSite,)>,
) {
    let (definition, _, semantic_key) = definitions.item();
    commands.insert((
        DefinitionSiteKey(definition),
        *semantic_key,
        DefinitionSiteRoot,
    ));
}

/// Updates the diagnostic payload on an existing stable definition site.
/// Navigation is the driver; the exact semantic key binds its sole site.
async fn enrich_definition_sites(
    navigation: Query<(
        Entity,
        &DefinitionNavigation,
        &DefinitionNameKey,
        &crate::entities::expansion::SemanticDefinitionKey,
    )>,
    sites: Query<
        (Entity, &DefinitionSiteRoot),
        Where<BowlEq<crate::entities::expansion::SemanticDefinitionKey>>,
    >,
    mut commands: Commands<(dsql_schema::DefinitionSiteContext,)>,
) {
    let (navigation_projection, navigation, name_key, _) = navigation.item();
    let (site, _) = sites.item();
    commands.insert((
        DerivedFrom::new(navigation_projection),
        DefinitionSiteContextOf(site),
        DefinitionSiteContext {
            definition: navigation.definition,
            provider_scope: navigation.provider_scope.clone(),
            name_key: name_key.clone(),
            file: navigation.file,
            name_span: navigation.name_span,
            file_path: navigation.file_path.clone(),
        },
    ));
}

/// Materializes one visible same-name provider for one definition site.
///
/// The semantic provider is the driver. Exact [`DefinitionNameKey`] binding
/// reaches only same-name consumers, whose [`DefinitionSiteKey`] then binds the
/// dedicated relationship owner.
async fn bind_visible_definition_candidates(
    providers: Query<(
        Entity,
        &DefinitionSemantics,
        &DefinitionNameKey,
        &crate::entities::expansion::SemanticDefinitionKey,
    )>,
    consumers: Query<
        (
            Entity,
            &DefinitionSemantics,
            &DefinitionNameKey,
            &DefinitionSiteKey,
        ),
        Where<BowlEq<DefinitionNameKey>>,
    >,
    sites: Query<(Entity, &DefinitionSiteRoot), Where<BowlEq<DefinitionSiteKey>>>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::VisibleDefinitionCandidate,)>,
) {
    let (provider_projection, provider, _, _) = providers.item();
    let (consumer_projection, consumer, _, _) = consumers.item();
    let (site, _) = sites.item();
    let (_, imports) = imports.item();
    if provider.definition == consumer.definition
        || !imports
            .visible_from(&consumer.provider_scope)
            .any(|scope| scope == provider.provider_scope)
    {
        return;
    }
    commands.insert((
        DerivedFrom::many([consumer_projection, provider_projection]),
        VisibleDefinitionCandidateOf(site),
        VisibleDefinitionCandidate {
            provider: provider.definition,
            provider_scope: provider.provider_scope.clone(),
        },
    ));
}

/// Reports same-scope duplicates and local/imported collisions from one
/// definition site's exact relationship-owned provider set.
async fn check_definition_conflicts(
    _: Query<Entity, With<DiagnosticsDemand>>,
    sites: Query<(
        Entity,
        &DefinitionSiteRoot,
        DefinitionSiteContextRows<'_>,
        DefinitionCandidateRows<'_>,
    )>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (_, _, contexts, candidates) = sites.item();
    let Some((context_entity, (consumer,))) = contexts.iter().next() else {
        return;
    };
    let mut candidates = candidates
        .iter()
        .map(|(entity, (candidate,))| (entity, *candidate))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.1
            .provider_scope
            .cmp(&right.1.provider_scope)
            .then_with(|| left.1.provider.cmp(&right.1.provider))
    });

    if let Some((candidate_entity, _)) = candidates.iter().find(|(_, candidate)| {
        candidate.provider_scope == consumer.provider_scope
            && candidate.provider < consumer.definition
    }) {
        let noun = match consumer.name_key.kind {
            DefKind::Query => "operation",
            DefKind::Fragment => "fragment",
        };
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([context_entity, *candidate_entity]),
                file: consumer.file,
                span: consumer.name_span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!("duplicate {noun} `{}`", consumer.name_key.name),
            },
        );
    }

    if let Some((candidate_entity, imported)) = candidates
        .iter()
        .find(|(_, candidate)| candidate.provider_scope != consumer.provider_scope)
    {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([context_entity, *candidate_entity]),
                file: consumer.file,
                span: consumer.name_span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!(
                    "{} `{}` collides with a definition imported from scope `{}`",
                    consumer.name_key.kind, consumer.name_key.name, imported.provider_scope
                ),
            },
        );
    }
}

/// Relates each ordered pair of same-name query definitions, carrying only the
/// consumer scopes that directly import both providers. The usual group size
/// is one, so exact name binding avoids the old query × scope materialization.
/// Navigation drives the binder because the diagnostic intentionally follows
/// movement of both the emitting and non-emitting providers.
async fn bind_imported_query_peers(
    providers: Query<(Entity, &DefinitionNavigation, &DefinitionNameKey)>,
    consumers: Query<
        (
            Entity,
            &DefinitionNavigation,
            &DefinitionNameKey,
            &DefinitionSiteKey,
        ),
        Where<BowlEq<DefinitionNameKey>>,
    >,
    sites: Query<(Entity, &DefinitionSiteRoot), Where<BowlEq<DefinitionSiteKey>>>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::ImportedQueryPeer,)>,
) {
    let (provider_projection, provider, name_key) = providers.item();
    if name_key.kind != DefKind::Query {
        return;
    }
    let (consumer_projection, consumer, _, _) = consumers.item();
    if provider.definition == consumer.definition {
        return;
    }
    let (site, _) = sites.item();
    let (_, imports) = imports.item();
    let consumer_scopes = imports
        .0
        .keys()
        .filter(|scope| {
            let imported = imports.imports_of(scope).collect::<Vec<_>>();
            imported
                .iter()
                .any(|scope| *scope == provider.provider_scope)
                && imported
                    .iter()
                    .any(|scope| *scope == consumer.provider_scope)
        })
        .cloned()
        .collect::<Vec<_>>();
    if consumer_scopes.is_empty() {
        return;
    }
    commands.insert((
        DerivedFrom::many([provider_projection, consumer_projection]),
        ImportedQueryPeerOf(site),
        ImportedQueryPeer {
            peer: provider.definition,
            peer_scope: provider.provider_scope.clone(),
            peer_path: provider.file_path.clone(),
            peer_name_span: provider.name_span,
            consumer_scopes,
        },
    ));
}

#[derive(Clone, Copy)]
struct ImportedQueryProvider<'a> {
    anchor: Entity,
    definition: Entity,
    scope: &'a str,
    path: &'a str,
    name_span: Span,
}

impl ImportedQueryProvider<'_> {
    fn order_key(&self) -> (&str, &str, usize, usize) {
        (
            self.scope,
            self.path,
            self.name_span.start,
            self.name_span.end,
        )
    }
}

/// Reports once per consumer scope and query name, on the deterministic first
/// provider in semantic/navigation order.
async fn check_import_ambiguities(
    _: Query<Entity, With<DiagnosticsDemand>>,
    sites: Query<(
        Entity,
        &DefinitionSiteRoot,
        DefinitionSiteContextRows<'_>,
        ImportedQueryRows<'_>,
    )>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (_, _, contexts, peers) = sites.item();
    let Some((context_entity, (context,))) = contexts.iter().next() else {
        return;
    };
    if context.name_key.kind != DefKind::Query {
        return;
    }
    let self_provider = ImportedQueryProvider {
        anchor: context_entity,
        definition: context.definition,
        scope: &context.provider_scope,
        path: &context.file_path,
        name_span: context.name_span,
    };
    let mut by_consumer = BTreeMap::<&str, Vec<ImportedQueryProvider<'_>>>::new();
    for (peer_entity, (peer,)) in peers.iter() {
        for consumer in &peer.consumer_scopes {
            by_consumer
                .entry(consumer)
                .or_default()
                .push(ImportedQueryProvider {
                    anchor: peer_entity,
                    definition: peer.peer,
                    scope: &peer.peer_scope,
                    path: &peer.peer_path,
                    name_span: peer.peer_name_span,
                });
        }
    }
    for (consumer, mut providers) in by_consumer {
        providers.push(self_provider);
        providers.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        providers.dedup_by_key(|provider| provider.definition);
        let distinct = providers
            .iter()
            .map(|provider| provider.scope)
            .collect::<std::collections::BTreeSet<_>>();
        if distinct.len() < 2 {
            continue;
        }
        let Some(first) = providers.first() else {
            continue;
        };
        if first.order_key() != self_provider.order_key() {
            continue;
        }
        let scopes = distinct.into_iter().collect::<Vec<_>>().join("`, `");
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many(providers.iter().map(|provider| provider.anchor)),
                file: context.file,
                span: context.name_span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!(
                    "query `{}` is provided to scope `{consumer}` by scopes `{scopes}`",
                    context.name_key.name
                ),
            },
        );
    }
}

impl FormatStage for Definition {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        if formatter.rule(node) == Some(Rule::QueryDef) {
            formatter.write_str("query");
            if let Some(name) = formatter.direct_name_text(node) {
                formatter.write_str(" ");
                formatter.write_str(&name);
            }
            if let Some(header) = formatter.direct_rule(node, Rule::QueryHeader) {
                formatter.definition_header(header);
            }
            for directive in formatter.direct_rules(node, Rule::Directive) {
                formatter.format_child(directive);
            }
        } else {
            formatter.write_str("fragment");
            if let Some(name) = formatter.direct_name_text(node) {
                formatter.write_str(" ");
                formatter.write_str(&name);
            }
            if let Some(header) = formatter.direct_rule(node, Rule::FragmentHeader) {
                formatter.definition_header(header);
            }
            formatter.write_str(" on");
            if let Some(on) = formatter.direct_qualified_name_text(node) {
                formatter.write_str(" ");
                formatter.write_str(&on);
            }
        }
        if let Some(selection_set) = formatter.direct_rule(node, Rule::SelectionSet) {
            formatter.selection_set(selection_set);
        }
    }
}

/// Answers hover on a definition name with its kind and target: one
/// invocation per (request, definition-in-file) pair via the
/// `BelongsToFile` join, the fragment target riding the definition row as
/// an optional part.
/// One definition row in the hovered file: the declaration with its
/// optional fragment target riding along.
type DefInFile<'a> = (Entity, &'a DefDecl, &'a NodeKey, Option<&'a FragmentTarget>);

/// Optional variable aggregate for the definition row currently paired by
/// [`NodeKey`]. It is absent when variable analysis was not demanded.
type VariablesForDefinition<'a> =
    Option<Query<(Entity, &'a DefinitionVariables), Where<BowlEq<NodeKey>>>>;

/// Optional query plan for the definition row currently paired by [`NodeKey`].
/// It is absent when plan demand is not armed or the query cannot be planned.
type PlanForDefinition<'a> = Option<Query<(Entity, &'a QueryPlanFact), Where<BowlEq<NodeKey>>>>;

async fn hover_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    defs: Query<DefInFile<'_>, Where<BowlEq<BelongsToFile>>>,
    variables: VariablesForDefinition<'_>,
    plan: PlanForDefinition<'_>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_def_entity, decl, _key, target) = defs.item();

    if !decl.name_span.contains(cursor.0) {
        return;
    }

    let dynamic_inputs = plan
        .map(|plan| plan.item().1.0.dynamic_inputs.as_slice())
        .unwrap_or_default();
    let (priority, text) = match (decl.kind, target, variables) {
        (DefKind::Query, _, Some(variables)) => {
            let (_, variables) = variables.item();
            (
                priority::QUERY_SIGNATURE,
                describe_query_variables(&decl.name, variables, dynamic_inputs),
            )
        }
        (DefKind::Query, _, None) => (priority::DEFINITION, format!("query `{}`", decl.name)),
        (DefKind::Fragment, Some(target), _) => (
            priority::DEFINITION,
            format!("fragment `{}` on `{}`", decl.name, target.name),
        ),
        (DefKind::Fragment, None, _) => (priority::DEFINITION, format!("fragment `{}`", decl.name)),
    };

    emit_hover_candidate(&mut commands, request, priority, text);
}

fn describe_query_variables(
    name: &str,
    variables: &DefinitionVariables,
    dynamic_inputs: &[DynamicInputContract],
) -> String {
    let shape = variable_shape(&variables.bindings, dynamic_inputs);
    if shape.is_empty() {
        format!("### Query `{name}`\n\nNo variables.")
    } else {
        format!("### Query `{name}`\n\n#### Variables\n\n```yaml\n{shape}```")
    }
}

#[derive(Default)]
struct VariableShapeNode {
    children: BTreeMap<String, VariableShapeNode>,
    value: Option<VariableShapeValue>,
}

enum VariableShapeValue {
    Scalar(String),
    Dynamic {
        binding: Box<VariableBinding>,
        contract: DynamicInputContract,
    },
}

fn variable_shape(bindings: &[VariableBinding], dynamic_inputs: &[DynamicInputContract]) -> String {
    let mut root = VariableShapeNode::default();
    for binding in bindings {
        let value = dynamic_contract(binding, dynamic_inputs).map_or_else(
            || VariableShapeValue::Scalar(variable_type_label(binding)),
            |contract| VariableShapeValue::Dynamic {
                binding: Box::new(binding.clone()),
                contract: contract.clone(),
            },
        );
        insert_variable_shape(
            &mut root,
            &binding.path.split('.').collect::<Vec<_>>(),
            value,
        );
    }
    let mut output = String::new();
    render_variable_shape(&root, 0, &mut output);
    output
}

fn dynamic_contract<'a>(
    binding: &VariableBinding,
    dynamic_inputs: &'a [DynamicInputContract],
) -> Option<&'a DynamicInputContract> {
    let kind = match binding.role {
        VariableRole::DynamicPredicate => DynamicInputKind::Predicate,
        VariableRole::DynamicOrder => DynamicInputKind::Order,
        _ => return None,
    };
    dynamic_inputs
        .iter()
        .find(|contract| contract.path == binding.path && contract.kind == kind)
}

pub(crate) fn describe_dynamic_variable(
    binding: &VariableBinding,
    dynamic_inputs: &[DynamicInputContract],
    binding_time: &str,
) -> Option<String> {
    dynamic_contract(binding, dynamic_inputs)?;
    let label = binding
        .name
        .as_deref()
        .map(|name| format!("`{name}`"))
        .unwrap_or_else(|| "anonymous variable".to_string());
    let shape = variable_shape(std::slice::from_ref(binding), dynamic_inputs);
    Some(format!(
        "{label} — `{}` ({binding_time})\n\n```yaml\n{shape}```",
        binding.path
    ))
}

fn insert_variable_shape(node: &mut VariableShapeNode, path: &[&str], value: VariableShapeValue) {
    let Some((head, tail)) = path.split_first() else {
        node.value = Some(value);
        return;
    };
    insert_variable_shape(
        node.children.entry((*head).to_string()).or_default(),
        tail,
        value,
    );
}

fn render_variable_shape(node: &VariableShapeNode, indent: usize, output: &mut String) {
    for (key, child) in &node.children {
        output.push_str(&"  ".repeat(indent));
        output.push_str(key);
        if child.children.is_empty() {
            match &child.value {
                Some(VariableShapeValue::Scalar(value)) => {
                    output.push_str(": ");
                    output.push_str(value);
                    output.push('\n');
                }
                Some(VariableShapeValue::Dynamic { binding, contract }) => {
                    render_dynamic_shape(binding, contract, indent, output);
                }
                None => output.push_str(": unknown\n"),
            }
        } else {
            output.push_str(":\n");
            if let Some(VariableShapeValue::Scalar(value)) = &child.value {
                output.push_str(&"  ".repeat(indent + 1));
                output.push_str("value: ");
                output.push_str(value);
                output.push('\n');
            }
            render_variable_shape(child, indent + 1, output);
        }
    }
}

fn render_dynamic_shape(
    binding: &VariableBinding,
    contract: &DynamicInputContract,
    indent: usize,
    output: &mut String,
) {
    output.push(':');
    let mut annotations = Vec::new();
    if contract.kind == DynamicInputKind::Order {
        annotations.push("ordered array of one-field entries".to_string());
    }
    if binding.nullable {
        annotations.push("nullable".to_string());
    }
    if let Some(default) = &binding.default {
        annotations.push(format!("default {}", input_default_label(default)));
    }
    if !annotations.is_empty() {
        output.push_str(" # ");
        output.push_str(&annotations.join("; "));
    }
    output.push('\n');

    match contract.kind {
        DynamicInputKind::Predicate => {
            write_shape_line(output, indent + 1, "and", "[<predicate>]");
            write_shape_line(output, indent + 1, "or", "[<predicate>]");
            write_shape_line(output, indent + 1, "not", "<predicate>");
            for field in &contract.fields {
                output.push_str(&"  ".repeat(indent + 1));
                output.push_str(&field.key);
                output.push_str(":\n");
                for operator in &field.operators {
                    write_shape_line(
                        output,
                        indent + 2,
                        operator.as_str(),
                        &dynamic_operand_type(field.data_type.as_str(), *operator),
                    );
                }
            }
        }
        DynamicInputKind::Order => {
            for field in &contract.fields {
                let directions = format!(
                    "enum({})",
                    field
                        .directions
                        .iter()
                        .map(|direction| format!("\"{}\"", direction.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                write_shape_line(output, indent + 1, &field.key, &directions);
            }
        }
    }
}

fn dynamic_operand_type(data_type: &str, operator: DynamicPredicateOperator) -> String {
    match operator {
        DynamicPredicateOperator::In | DynamicPredicateOperator::NotIn => {
            format!("{data_type}[]")
        }
        DynamicPredicateOperator::IsNull => "boolean".to_string(),
        _ => data_type.to_string(),
    }
}

fn write_shape_line(output: &mut String, indent: usize, key: &str, value: &str) {
    output.push_str(&"  ".repeat(indent));
    output.push_str(key);
    output.push_str(": ");
    output.push_str(value);
    output.push('\n');
}
