//! Explicit trusted-context declarations and their scope-aware resolution.
//!
//! Declaration projections and exact use-site relationships are the source of
//! trusted-context types for language semantics and editor services.
//! [`ContextIndex`] remains temporarily as a policy-compiler bridge; the policy
//! registry slice removes that final persistent aggregation.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Phase, Query, Registrar, Related,
    SystemExt, TrackedView, View, Where, With,
};

use crate::catalog::{Catalog, CatalogSnapshot, CatalogTypeShape, DataType, TypeKey, WireEncoding};
use crate::entities::definition::DefIndex;
use crate::entities::expression::Sigil;
use crate::entities::variable::VariableUse;
use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    SemanticMemberOf, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{
    CompletionContext, CompletionItem, CompletionKind, CompletionRequest, CompletionSite,
    emit_completion_candidate,
};
use crate::service::definition::{DefinitionRequest, DefinitionTarget};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::source::{BelongsToHost, FilePath, ResolutionScope, ScopeImports};

/// One entry lowered from a scope-level `context` declaration block.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ContextDecl {
    /// Trusted-context key without the generated `context.` prefix.
    pub name: String,
    /// Precise source span of [`ContextDecl::name`].
    pub name_span: Span,
    /// Optional provider schema. Built-in logical types remain unqualified.
    pub type_schema: Option<String>,
    /// Built-in logical name or provider-internal type name.
    pub type_name: String,
    /// Precise source span of the complete type, including collection suffix.
    pub type_span: Span,
    /// Whether the declaration accepts a collection of the named scalar type.
    pub collection: bool,
}

/// Stable declaration identity shared by context projections and services.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationKey(pub Entity);

/// Exact name bucket used to bind declarations and context uses.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextNameKey(pub String);

/// Source-only context declaration metadata lowered with the syntax fact.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextSource {
    pub path: String,
    pub embedded: bool,
}

/// Catalog-resolved declaration semantics without source navigation data.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationSemantics {
    pub declaration: Entity,
    pub provider_scope: String,
    pub contract: Option<ContextValueContract>,
    pub problem: Option<ContextTypeProblem>,
}

/// Source location and ordering data for one context declaration.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationNavigation {
    pub declaration: Entity,
    pub file: Entity,
    pub provider_scope: String,
    pub file_path: String,
    pub name_span: Span,
    pub type_span: Span,
    pub embedded: bool,
}

/// Stable key shared by one declaration and its relationship owner.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationSiteKey(pub Entity);

/// Stable owner for one declaration's diagnostics and same-name peers.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationSiteRoot;

/// Complete declaration payload attached to its stable site.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationContext {
    pub declaration: Entity,
    pub file: Entity,
    pub provider_scope: String,
    pub file_path: String,
    pub name: String,
    pub name_span: Span,
    pub type_span: Span,
    pub contract: Option<ContextValueContract>,
    pub problem: Option<ContextTypeProblem>,
    pub embedded: bool,
}

/// Relationship edge from declaration context to its stable site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ContextDeclarationContexts)]
pub struct ContextDeclarationContextOf(pub Entity);

/// Engine-maintained declaration context for one stable site.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ContextDeclarationContextOf)]
pub struct ContextDeclarationContexts(pub Vec<Entity>);

/// One same-name declaration peer, including effective shared consumers.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextDeclarationPeer {
    pub peer: Entity,
    pub peer_scope: String,
    pub peer_path: String,
    pub peer_name_span: Span,
    pub consumer_scopes: Vec<String>,
}

/// Relationship edge from a same-name peer to one declaration site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ContextDeclarationPeers)]
pub struct ContextDeclarationPeerOf(pub Entity);

/// Engine-maintained same-name declaration peers for one site.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ContextDeclarationPeerOf)]
pub struct ContextDeclarationPeers(pub Vec<Entity>);

/// Authoritative value contract resolved from one [`ContextDecl`].
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ContextValueContract {
    pub data_type: DataType,
    pub wire: WireEncoding,
    pub provider_type: Option<TypeKey>,
    pub collection: bool,
    pub closed_values: Vec<String>,
}

/// Stable type-resolution failure attached to one indexed declaration.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ContextTypeProblem {
    UnknownBuiltin { name: String },
    UnknownProvider { key: TypeKey },
    ProviderArray { key: TypeKey },
    UnsupportedWire { name: String },
}

/// One context entry after catalog resolution, including its source target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextEntry {
    pub declaration: Entity,
    pub file: Entity,
    pub file_path: String,
    pub scope: String,
    pub name: String,
    pub name_span: Span,
    pub type_span: Span,
    pub contract: Option<ContextValueContract>,
    pub problem: Option<ContextTypeProblem>,
    pub embedded: bool,
}

/// Temporary policy-only index of every trusted-context declaration.
///
/// [`crate::entities::policy::compile_policies`] is the sole remaining
/// consumer. The policy registry slice removes this aggregate and its
/// [`DefIndex`] host.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct ContextIndex {
    pub entries: Vec<ContextEntry>,
}

impl Hash for ContextIndex {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for entry in &self.entries {
            // The source entity is stable across text edits but changes when a
            // document is replaced, so it keeps navigation targets current.
            // The lowered declaration entity is deliberately omitted: it is
            // re-minted on any re-lowering and would invalidate every context
            // consumer after unrelated edits in the declaring file.
            entry.file.hash(state);
            entry.file_path.hash(state);
            entry.scope.hash(state);
            entry.name.hash(state);
            entry.name_span.hash(state);
            entry.type_span.hash(state);
            entry.contract.hash(state);
            entry.problem.hash(state);
            entry.embedded.hash(state);
        }
    }
}

/// Result of resolving one context name in an effective scope.
pub(crate) enum ContextLookup<'a> {
    Resolved(&'a ContextValueContract),
    Unknown,
    Ambiguous,
    Invalid,
}

impl ContextIndex {
    /// All declarations visible from `scope` with exactly `name`.
    pub fn visible<'a>(
        &'a self,
        scope: &str,
        name: &str,
        imports: &'a ScopeImports,
    ) -> Vec<&'a ContextEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.name == name
                    && imports
                        .visible_from(scope)
                        .any(|visible| visible == entry.scope)
            })
            .collect()
    }

    /// Resolves one name only when the effective scope supplies one valid entry.
    pub(crate) fn lookup<'a>(
        &'a self,
        scope: &str,
        name: &str,
        imports: &'a ScopeImports,
    ) -> ContextLookup<'a> {
        let visible = self.visible(scope, name, imports);
        match visible.as_slice() {
            [] => ContextLookup::Unknown,
            [entry] => entry
                .contract
                .as_ref()
                .map_or(ContextLookup::Invalid, ContextLookup::Resolved),
            _ => ContextLookup::Ambiguous,
        }
    }
}

/// Source navigation and declaration contract for one context occurrence.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedContextUse {
    /// Source [`VariableUse`] this resolution describes.
    pub source_use: Entity,
    pub name: String,
    pub span: Span,
    pub resolution: ContextUseResolution,
}

/// Relationship edge from a context-use resolution to its semantic group.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ContextUseResolutions)]
pub struct ContextUseResolutionOf(pub Entity);

/// Engine-maintained context-use resolutions owned by one semantic group.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ContextUseResolutionOf)]
pub struct ContextUseResolutions(pub Vec<Entity>);

/// Stable key shared by one context occurrence and its relationship site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextUseSiteKey(pub Entity);

/// Stable owner for one context occurrence's visible declarations.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextUseSiteRoot;

/// Local syntax and scope payload for one context occurrence.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextUseContext {
    pub source_use: Entity,
    pub name: String,
    pub span: Span,
    pub file: Entity,
    pub scope: String,
    pub semantic_group: Entity,
}

/// Relationship edge from a use payload to its stable site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ContextUseContexts)]
pub struct ContextUseContextOf(pub Entity);

/// Engine-maintained local payload for one context occurrence.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ContextUseContextOf)]
pub struct ContextUseContexts(pub Vec<Entity>);

/// One same-name declaration visible from a context occurrence.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct ContextUseCandidate {
    pub declaration: Entity,
    pub provider_scope: String,
    pub contract: Option<ContextValueContract>,
}

/// Relationship edge from a visible declaration to one context-use site.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(untracked)]
#[relationship(target = ContextUseCandidates)]
pub struct ContextUseCandidateOf(pub Entity);

/// Engine-maintained visible declarations for one context occurrence.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ContextUseCandidateOf)]
pub struct ContextUseCandidates(pub Vec<Entity>);

/// Scope lookup outcome for one [`ResolvedContextUse`].
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ContextUseResolution {
    Resolved {
        declaration: Entity,
        contract: ContextValueContract,
    },
    Unknown,
    Ambiguous {
        providers: Vec<String>,
    },
    Invalid,
}

/// Validates one use-site expectation without replacing declaration semantics.
pub(crate) fn validate_context_use(
    name: &str,
    declared: &ContextValueContract,
    expected: &ContextValueContract,
) -> Result<Vec<String>, String> {
    let type_matches =
        if declared.wire == WireEncoding::TextCast || expected.wire == WireEncoding::TextCast {
            declared.wire == expected.wire && declared.provider_type == expected.provider_type
        } else {
            declared.data_type == expected.data_type && declared.wire == expected.wire
        };
    if !type_matches || declared.collection != expected.collection {
        return Err(format!(
            "trusted context `context.{name}` is declared as `{}` but this use requires `{}`",
            context_type_label(declared),
            context_type_label(expected),
        ));
    }

    if expected.closed_values.is_empty() {
        return Ok(declared.closed_values.clone());
    }
    if declared.closed_values.is_empty()
        || expected
            .closed_values
            .iter()
            .all(|value| declared.closed_values.contains(value))
    {
        return Ok(expected.closed_values.clone());
    }
    Err(format!(
        "trusted context `context.{name}` does not allow every value required by this use"
    ))
}

/// Human-facing logical/provider label for one resolved context contract.
pub(crate) fn context_type_label(contract: &ContextValueContract) -> String {
    let base = contract.provider_type.as_ref().map_or_else(
        || contract.data_type.as_str().to_string(),
        |key| format!("{}::{}", key.schema, key.name),
    );
    if contract.collection {
        format!("{base}[]")
    } else {
        base
    }
}

/// Owns `context_def` and all declaration-driven context semantics.
pub struct Context;

impl LanguageEntity for Context {
    const NAME: &'static str = "context";

    fn register(registrar: &mut Registrar<'_>) {
        registrar.system(project_context_declarations);
        registrar.system(project_context_declaration_sites);
        registrar.system(enrich_context_declaration_sites);
        registrar.system(bind_context_declaration_peers);
        registrar.system(project_context_use_sites);
        registrar.system(enrich_context_use_sites);
        registrar.system(bind_context_use_candidates);
        // Temporary policy-only bridge. The policy registry slice removes
        // this final persistent context aggregation and its DefIndex host.
        registrar.system(index_contexts.run_during(Phase::Complete));
        registrar.system(resolve_context_uses);
        registrar.system(check_context_declarations);
        registrar.system(check_context_uses);
        registrar.system(hover_context_declarations);
        registrar.system(hover_context_uses);
        registrar.system(complete_context_uses.run_during(Phase::Complete));
        registrar.system(define_context_uses);
    }
}

impl LowerStage for Context {
    fn lower(
        context: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        for entry in context
            .cst
            .children(node)
            .filter(|child| context.cst.match_rule(*child, Rule::ContextEntry))
        {
            let Some(name_span) = direct_name(context.cst, entry) else {
                continue;
            };
            let Some(context_type) = direct_rule(context.cst, entry, Rule::ContextType) else {
                continue;
            };
            let Some(qualified) = direct_rule(context.cst, context_type, Rule::QualifiedName)
            else {
                continue;
            };
            let parts = crate::entities::direct_names(context.cst, qualified);
            let (type_schema, type_name) = match parts.as_slice() {
                [name] => (None, text(context.source, *name).to_string()),
                [schema, name] => (
                    Some(text(context.source, *schema).to_string()),
                    text(context.source, *name).to_string(),
                ),
                _ => continue,
            };
            let declaration = ContextDecl {
                name: text(context.source, name_span).to_string(),
                name_span,
                type_schema,
                type_name,
                type_span: node_span(context.cst, context_type),
                collection: context
                    .cst
                    .children(context_type)
                    .any(|child| context.cst.match_token(child, Token::LBracket).is_some()),
            };
            let name = declaration.name.clone();
            let entity = commands
                .insert((
                    DerivedFrom::new(context.file),
                    BelongsToFile(context.file),
                    NodeKey {
                        file: context.file,
                        node: entry.0,
                    },
                    ResolutionScope(context.scope.to_string()),
                    ContextNameKey(name),
                    ContextSource {
                        path: context.path.to_string(),
                        embedded: context.embedded,
                    },
                    declaration,
                ))
                .untyped();
            commands
                .entity(entity)
                .insert(ContextDeclarationKey(entity));
        }
        None
    }
}

impl FormatStage for Context {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.context_definition(node);
    }
}

type ContextDeclarationRows<'a> =
    Related<ContextDeclarationContexts, (&'a ContextDeclarationContext,)>;
type ContextDeclarationPeerRows<'a> =
    Related<ContextDeclarationPeers, (&'a ContextDeclarationPeer,)>;
type ContextUseRows<'a> = Related<ContextUseContexts, (&'a ContextUseContext,)>;
type ContextUseCandidateRows<'a> = Related<ContextUseCandidates, (&'a ContextUseCandidate,)>;
type ContextPeerProvider<'a> = Query<(
    Entity,
    &'a ContextDeclarationNavigation,
    &'a ContextNameKey,
    &'a ContextDeclarationKey,
)>;
type ContextPeerConsumer<'a> = Query<
    (
        Entity,
        &'a ContextDeclarationNavigation,
        &'a ContextNameKey,
        &'a ContextDeclarationKey,
        &'a ContextDeclarationSiteKey,
    ),
    Where<BowlEq<ContextNameKey>>,
>;

/// Projects one declaration into independent catalog semantics and navigation
/// products. Catalog changes can rerun this system without moving the
/// navigation projection or any navigation-driven peer.
async fn project_context_declarations(
    declarations: Query<(
        Entity,
        &ContextDecl,
        &ContextDeclarationKey,
        &ContextNameKey,
        &ContextSource,
        &BelongsToFile,
        &ResolutionScope,
    )>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(
        dsql_schema::ContextDeclarationSemanticProjection,
        dsql_schema::ContextDeclarationNavigationProjection,
    )>,
) {
    let (declaration, context, key, name_key, source, file, scope) = declarations.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (contract, problem) = resolve_contract(snapshot.catalog(), context);
    let site_key = ContextDeclarationSiteKey(declaration);
    commands.insert((
        DerivedFrom::many([declaration, catalog_entity]),
        *key,
        name_key.clone(),
        site_key,
        ContextDeclarationSemantics {
            declaration,
            provider_scope: scope.0.clone(),
            contract,
            problem,
        },
    ));
    commands.insert((
        DerivedFrom::new(declaration),
        *key,
        name_key.clone(),
        site_key,
        ContextDeclarationNavigation {
            declaration,
            file: file.0,
            provider_scope: scope.0.clone(),
            file_path: source.path.clone(),
            name_span: context.name_span,
            type_span: context.type_span,
            embedded: source.embedded,
        },
    ));
}

/// Creates one stable relationship owner per context declaration.
async fn project_context_declaration_sites(
    declarations: Query<(Entity, &ContextDecl, &ContextDeclarationKey)>,
    mut commands: Commands<(dsql_schema::ContextDeclarationSite,)>,
) {
    let (declaration, _, _) = declarations.item();
    commands.insert((
        ContextDeclarationSiteKey(declaration),
        ContextDeclarationSiteRoot,
    ));
}

/// Joins the exact semantic and navigation projections onto their stable site.
async fn enrich_context_declaration_sites(
    semantics: Query<(
        Entity,
        &ContextDeclarationSemantics,
        &ContextDeclarationKey,
        &ContextNameKey,
        &ContextDeclarationSiteKey,
    )>,
    navigation: Query<
        (
            Entity,
            &ContextDeclarationNavigation,
            &ContextDeclarationKey,
        ),
        Where<BowlEq<ContextDeclarationKey>>,
    >,
    sites: Query<(Entity, &ContextDeclarationSiteRoot), Where<BowlEq<ContextDeclarationSiteKey>>>,
    mut commands: Commands<(dsql_schema::ContextDeclarationContext,)>,
) {
    let (semantic_entity, semantic, declaration_key, name_key, _) = semantics.item();
    let (navigation_entity, navigation, _) = navigation.item();
    let (site, _) = sites.item();
    commands.insert((
        DerivedFrom::many([semantic_entity, navigation_entity]),
        ContextDeclarationContextOf(site),
        *declaration_key,
        name_key.clone(),
        ContextDeclarationContext {
            declaration: semantic.declaration,
            file: navigation.file,
            provider_scope: semantic.provider_scope.clone(),
            file_path: navigation.file_path.clone(),
            name: name_key.0.clone(),
            name_span: navigation.name_span,
            type_span: navigation.type_span,
            contract: semantic.contract.clone(),
            problem: semantic.problem.clone(),
            embedded: navigation.embedded,
        },
    ));
}

/// Relates ordered same-name declaration pairs. Navigation is deliberately
/// the driver because declaration diagnostics must follow provider movement.
async fn bind_context_declaration_peers(
    providers: ContextPeerProvider<'_>,
    consumers: ContextPeerConsumer<'_>,
    sites: Query<(Entity, &ContextDeclarationSiteRoot), Where<BowlEq<ContextDeclarationSiteKey>>>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::ContextDeclarationPeer,)>,
) {
    let (provider_entity, provider, _, provider_key) = providers.item();
    let (consumer_entity, consumer, _, consumer_key, _) = consumers.item();
    if provider_key == consumer_key {
        return;
    }
    let (site, _) = sites.item();
    let (_, imports) = imports.item();
    let consumer_scopes = imports
        .0
        .keys()
        .filter(|scope| {
            let effective_imports = imports.imports_of(scope).collect::<Vec<_>>();
            effective_imports
                .iter()
                .any(|scope| *scope == provider.provider_scope)
                && effective_imports
                    .iter()
                    .any(|scope| *scope == consumer.provider_scope)
        })
        .cloned()
        .collect::<Vec<_>>();
    // imports_of excludes the local scope but includes its complete recursive
    // import closure: declarations reachable elsewhere collide, while local
    // same-scope duplicates follow the separate branch below.
    let local_collision = imports
        .imports_of(&consumer.provider_scope)
        .any(|scope| scope == provider.provider_scope);
    if provider.provider_scope != consumer.provider_scope
        && !local_collision
        && consumer_scopes.is_empty()
    {
        return;
    }
    commands.insert((
        DerivedFrom::many([provider_entity, consumer_entity]),
        ContextDeclarationPeerOf(site),
        ContextDeclarationPeer {
            peer: provider.declaration,
            peer_scope: provider.provider_scope.clone(),
            peer_path: provider.file_path.clone(),
            peer_name_span: provider.name_span,
            consumer_scopes,
        },
    ));
}

/// Creates one stable relationship owner for every context-sigil occurrence.
async fn project_context_use_sites(
    uses: Query<(Entity, &VariableUse)>,
    mut commands: Commands<(dsql_schema::ContextUseSite,)>,
) {
    let (use_entity, variable) = uses.item();
    if variable.sigil() != Sigil::Context || variable.0.name.is_none() {
        return;
    }
    commands
        .entity(use_entity)
        .insert(ContextUseSiteKey(use_entity));
    commands.insert((ContextUseSiteKey(use_entity), ContextUseSiteRoot));
}

/// Attaches one occurrence's local syntax and scope to its stable site.
async fn enrich_context_use_sites(
    uses: Query<(
        Entity,
        &VariableUse,
        &BelongsToFile,
        &ResolutionScope,
        &SemanticMemberOf,
        &ContextUseSiteKey,
    )>,
    sites: Query<(Entity, &ContextUseSiteRoot), Where<BowlEq<ContextUseSiteKey>>>,
    mut commands: Commands<(dsql_schema::ContextUseContext,)>,
) {
    let (use_entity, variable, file, scope, semantic_group, _) = uses.item();
    if variable.sigil() != Sigil::Context {
        return;
    }
    let Some(name) = variable.0.name.as_deref() else {
        return;
    };
    let (site, _) = sites.item();
    commands.insert((
        DerivedFrom::new(use_entity),
        ContextUseContextOf(site),
        ContextUseSiteKey(use_entity),
        ContextNameKey(name.to_string()),
        ContextUseContext {
            source_use: use_entity,
            name: name.to_string(),
            span: variable.0.span,
            file: file.0,
            scope: scope.0.clone(),
            semantic_group: semantic_group.0,
        },
    ));
}

/// Relates only semantically visible same-name declarations to one use site.
/// No navigation component participates, so declaration span movement cannot
/// wake context inference, planning, or use resolution.
async fn bind_context_use_candidates(
    providers: Query<(
        Entity,
        &ContextDeclarationSemantics,
        &ContextNameKey,
        &ContextDeclarationKey,
    )>,
    uses: Query<
        (
            Entity,
            &ContextUseContext,
            &ContextNameKey,
            &ContextUseSiteKey,
        ),
        Where<BowlEq<ContextNameKey>>,
    >,
    sites: Query<(Entity, &ContextUseSiteRoot), Where<BowlEq<ContextUseSiteKey>>>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::ContextUseCandidate,)>,
) {
    let (provider_entity, provider, _, provider_key) = providers.item();
    let (use_context_entity, context, _, _) = uses.item();
    let (site, _) = sites.item();
    let (_, imports) = imports.item();
    if !imports
        .visible_from(&context.scope)
        .any(|scope| scope == provider.provider_scope)
    {
        return;
    }
    commands.insert((
        DerivedFrom::many([provider_entity, use_context_entity]),
        ContextUseCandidateOf(site),
        ContextUseCandidate {
            declaration: provider_key.0,
            provider_scope: provider.provider_scope.clone(),
            contract: provider.contract.clone(),
        },
    ));
}

async fn index_contexts(
    catalog: Query<(Entity, &CatalogSnapshot)>,
    definitions: Query<(Entity, &DefIndex)>,
    declarations: TrackedView<'_, (Entity, &ContextDecl, &BelongsToFile, &ResolutionScope)>,
    files: TrackedView<'_, (Entity, &FilePath)>,
    embedded: TrackedView<'_, (Entity, &BelongsToHost)>,
    mut commands: Commands<(dsql_schema::DefIndex,)>,
) {
    let (_, snapshot) = catalog.item();
    let file_paths = files
        .iter()
        .map(|(entity, path)| (entity, path.0.as_str()))
        .collect::<BTreeMap<_, _>>();
    let embedded_files = embedded
        .iter()
        .map(|(entity, _)| entity)
        .collect::<BTreeSet<_>>();
    let mut entries = declarations
        .iter()
        .map(|(entity, declaration, file, scope)| {
            let (contract, problem) = resolve_contract(snapshot.catalog(), declaration);
            ContextEntry {
                declaration: entity,
                file: file.0,
                file_path: file_paths
                    .get(&file.0)
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                scope: scope.0.clone(),
                name: declaration.name.clone(),
                name_span: declaration.name_span,
                type_span: declaration.type_span,
                contract,
                problem,
                embedded: embedded_files.contains(&file.0),
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.scope
            .cmp(&right.scope)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.name_span.start.cmp(&right.name_span.start))
    });
    commands
        .entity(definitions.item().0)
        .insert(ContextIndex { entries });
}

fn resolve_contract(
    catalog: &Catalog,
    declaration: &ContextDecl,
) -> (Option<ContextValueContract>, Option<ContextTypeProblem>) {
    if let Some(schema) = &declaration.type_schema {
        let key = TypeKey::new(schema, &declaration.type_name);
        let Some(data_type) = catalog.type_by_key(&key) else {
            return (None, Some(ContextTypeProblem::UnknownProvider { key }));
        };
        if matches!(data_type.shape, CatalogTypeShape::Array { .. }) {
            return (None, Some(ContextTypeProblem::ProviderArray { key }));
        }
        if data_type.capabilities.wire == WireEncoding::Unsupported {
            return (
                None,
                Some(ContextTypeProblem::UnsupportedWire {
                    name: format!("{}::{}", key.schema, key.name),
                }),
            );
        }
        let closed_values =
            catalog
                .enum_type_for_type(data_type.id)
                .map_or_else(Vec::new, |(_, enumeration)| {
                    enumeration
                        .variants
                        .iter()
                        .map(|variant| variant.variant.clone())
                        .collect()
                });
        return (
            Some(ContextValueContract {
                data_type: data_type.logical_data_type(),
                wire: data_type.capabilities.wire,
                provider_type: Some(key),
                collection: declaration.collection,
                closed_values,
            }),
            None,
        );
    }

    let Some(data_type) = Catalog::resolve_logical_type_name(&declaration.type_name) else {
        return (
            None,
            Some(ContextTypeProblem::UnknownBuiltin {
                name: declaration.type_name.clone(),
            }),
        );
    };
    let capabilities = Catalog::builtin_capabilities(data_type);
    if capabilities.wire == WireEncoding::Unsupported {
        return (
            None,
            Some(ContextTypeProblem::UnsupportedWire {
                name: declaration.type_name.clone(),
            }),
        );
    }
    (
        Some(ContextValueContract {
            data_type,
            wire: capabilities.wire,
            provider_type: None,
            collection: declaration.collection,
            closed_values: Vec::new(),
        }),
        None,
    )
}

async fn resolve_context_uses(
    sites: Query<(
        Entity,
        &ContextUseSiteRoot,
        ContextUseRows<'_>,
        ContextUseCandidateRows<'_>,
    )>,
    mut commands: Commands<(dsql_schema::ResolvedContextUse,)>,
) {
    let (_, _, contexts, candidates) = sites.item();
    let Some((_, (context,))) = contexts.iter().next() else {
        return;
    };
    let mut candidates = candidates
        .iter()
        .map(|(_, (candidate,))| candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.provider_scope
            .cmp(&right.provider_scope)
            .then_with(|| left.declaration.cmp(&right.declaration))
    });
    let (resolution, declaration_key) =
        match candidates.as_slice() {
            [] => (ContextUseResolution::Unknown, None),
            [candidate] => candidate.contract.as_ref().map_or(
                (ContextUseResolution::Invalid, None),
                |contract| {
                    (
                        ContextUseResolution::Resolved {
                            declaration: candidate.declaration,
                            contract: contract.clone(),
                        },
                        Some(ContextDeclarationKey(candidate.declaration)),
                    )
                },
            ),
            candidates => {
                let providers = candidates
                    .iter()
                    .map(|candidate| candidate.provider_scope.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                (ContextUseResolution::Ambiguous { providers }, None)
            }
        };
    let resolved = ResolvedContextUse {
        source_use: context.source_use,
        name: context.name.clone(),
        span: context.span,
        resolution,
    };
    if let Some(declaration_key) = declaration_key {
        commands.insert((
            BelongsToFile(context.file),
            ContextUseResolutionOf(context.semantic_group),
            declaration_key,
            resolved,
        ));
    } else {
        commands.insert((
            BelongsToFile(context.file),
            ContextUseResolutionOf(context.semantic_group),
            resolved,
        ));
    }
}

async fn check_context_declarations(
    _: Query<Entity, With<DiagnosticsDemand>>,
    sites: Query<(
        Entity,
        &ContextDeclarationSiteRoot,
        ContextDeclarationRows<'_>,
        ContextDeclarationPeerRows<'_>,
    )>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (_, _, contexts, peers) = sites.item();
    let Some((context_entity, (context,))) = contexts.iter().next() else {
        return;
    };
    let (_, imports) = imports.item();

    if context.embedded {
        emit_context_diagnostic(
            &mut commands,
            DerivedFrom::new(context_entity),
            context,
            context.name_span,
            DiagnosticCode::InvalidContextDefinition,
            "context declarations must be standalone DSQL definitions".to_string(),
        );
    }
    if let Some(problem) = &context.problem {
        emit_context_diagnostic(
            &mut commands,
            DerivedFrom::new(context_entity),
            context,
            context.type_span,
            DiagnosticCode::InvalidContextDefinition,
            context_problem_message(problem),
        );
    }

    let mut peers = peers.iter().collect::<Vec<_>>();
    peers.sort_by(|(_, (left,)), (_, (right,))| {
        context_peer_order(left).cmp(&context_peer_order(right))
    });
    let self_order = context_declaration_order(context);

    if let Some((peer_entity, _)) = peers.iter().find(|(_, (peer,))| {
        peer.peer_scope == context.provider_scope && context_peer_order(peer) < self_order
    }) {
        emit_context_diagnostic(
            &mut commands,
            DerivedFrom::many([context_entity, *peer_entity]),
            context,
            context.name_span,
            DiagnosticCode::DuplicateDefinition,
            format!("duplicate context entry `{}`", context.name),
        );
    }

    if let Some((peer_entity, (imported,))) = peers.iter().find(|(_, (peer,))| {
        imports
            .imports_of(&context.provider_scope)
            .any(|provider| provider == peer.peer_scope)
    }) {
        emit_context_diagnostic(
            &mut commands,
            DerivedFrom::many([context_entity, *peer_entity]),
            context,
            context.name_span,
            DiagnosticCode::DuplicateDefinition,
            format!(
                "context entry `{}` collides with a declaration imported from scope `{}`",
                context.name, imported.peer_scope
            ),
        );
    }

    let mut by_consumer = BTreeMap::<&str, Vec<(Entity, &ContextDeclarationPeer)>>::new();
    for (peer_entity, (peer,)) in &peers {
        for consumer in &peer.consumer_scopes {
            by_consumer
                .entry(consumer)
                .or_default()
                .push((*peer_entity, peer));
        }
    }
    for (consumer, consumer_peers) in by_consumer {
        let mut providers = consumer_peers
            .iter()
            .map(|(_, peer)| peer.peer_scope.as_str())
            .collect::<BTreeSet<_>>();
        providers.insert(&context.provider_scope);
        if providers.len() < 2 {
            continue;
        }
        let first_is_self = consumer_peers
            .iter()
            .all(|(_, peer)| self_order <= context_peer_order(peer));
        if !first_is_self {
            continue;
        }
        emit_context_diagnostic(
            &mut commands,
            DerivedFrom::many(
                std::iter::once(context_entity)
                    .chain(consumer_peers.iter().map(|(entity, _)| *entity)),
            ),
            context,
            context.name_span,
            DiagnosticCode::AmbiguousTrustedContext,
            format!(
                "context entry `{}` is provided to scope `{consumer}` by scopes `{}`",
                context.name,
                providers.into_iter().collect::<Vec<_>>().join("`, `")
            ),
        );
    }
}

fn context_declaration_order(context: &ContextDeclarationContext) -> (&str, &str, usize, usize) {
    (
        &context.provider_scope,
        &context.file_path,
        context.name_span.start,
        context.name_span.end,
    )
}

fn context_peer_order(peer: &ContextDeclarationPeer) -> (&str, &str, usize, usize) {
    (
        &peer.peer_scope,
        &peer.peer_path,
        peer.peer_name_span.start,
        peer.peer_name_span.end,
    )
}

fn context_problem_message(problem: &ContextTypeProblem) -> String {
    match problem {
        ContextTypeProblem::UnknownBuiltin { name } => format!(
            "unknown built-in context type `{name}`; catalog/provider types must be schema-qualified"
        ),
        ContextTypeProblem::UnknownProvider { key } => {
            format!("provider type `{}::{}` not found", key.schema, key.name)
        }
        ContextTypeProblem::ProviderArray { key } => format!(
            "provider array type `{}::{}` cannot be declared directly; declare its element type with `[]`",
            key.schema, key.name
        ),
        ContextTypeProblem::UnsupportedWire { name } => {
            format!("context type `{name}` has no supported input wire encoding")
        }
    }
}

fn emit_context_diagnostic(
    commands: &mut Commands<(dsql_schema::Diagnostic,)>,
    derived_from: DerivedFrom,
    entry: &ContextDeclarationContext,
    span: Span,
    code: DiagnosticCode,
    message: String,
) {
    emit_diagnostic(
        commands,
        DiagnosticFacts {
            derived_from,
            file: entry.file,
            span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code,
            message,
        },
    );
}

async fn check_context_uses(
    _: Query<Entity, With<DiagnosticsDemand>>,
    uses: Query<(Entity, &ResolvedContextUse, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, resolved, file) = uses.item();
    let (code, message) = match &resolved.resolution {
        ContextUseResolution::Unknown => (
            DiagnosticCode::UnknownTrustedContext,
            format!("trusted context `{}` is not declared", resolved.name),
        ),
        ContextUseResolution::Ambiguous { providers } => (
            DiagnosticCode::AmbiguousTrustedContext,
            format!(
                "trusted context `{}` is ambiguous across scopes `{}`",
                resolved.name,
                providers.join("`, `")
            ),
        ),
        ContextUseResolution::Resolved { .. } | ContextUseResolution::Invalid => return,
    };
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(entity),
            file: file.0,
            span: resolved.span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code,
            message,
        },
    );
}

async fn hover_context_declarations(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    declaration: Query<
        (Entity, &ContextDecl, &ContextDeclarationKey),
        Where<BowlEq<BelongsToFile>>,
    >,
    semantics: Query<
        (Entity, &ContextDeclarationSemantics, &ContextDeclarationKey),
        Where<BowlEq<ContextDeclarationKey>>,
    >,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, declaration, _) = declaration.item();
    if !declaration.name_span.contains(cursor.0) && !declaration.type_span.contains(cursor.0) {
        return;
    }
    let (_, semantics, _) = semantics.item();
    let Some(contract) = &semantics.contract else {
        return;
    };
    emit_hover_candidate(
        &mut commands,
        request,
        priority::VARIABLE,
        format!(
            "`{}` — `context.{}`: {} (trusted context, required)",
            declaration.name,
            declaration.name,
            context_type_label(contract)
        ),
    );
}

async fn hover_context_uses(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    uses: Query<(Entity, &ResolvedContextUse), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, resolved) = uses.item();
    if !resolved.span.contains(cursor.0) {
        return;
    }
    let ContextUseResolution::Resolved { contract, .. } = &resolved.resolution else {
        return;
    };
    emit_hover_candidate(
        &mut commands,
        request,
        priority::VARIABLE,
        format!(
            "`{}` — `context.{}`: {} (trusted context, required)",
            resolved.name,
            resolved.name,
            context_type_label(contract)
        ),
    );
}

async fn complete_context_uses(
    request: Query<(Entity, &CompletionContext), With<CompletionRequest>>,
    declarations: View<'_, (Entity, &ContextDeclarationContext)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    let (request, context) = request.item();
    if context.site != CompletionSite::ContextVariable {
        return;
    }
    let (_, imports) = imports.item();
    let mut by_name = BTreeMap::<&str, Vec<&ContextDeclarationContext>>::new();
    for (_, entry) in declarations.iter() {
        if imports
            .visible_from(&context.scope)
            .any(|visible| visible == entry.provider_scope)
        {
            by_name.entry(&entry.name).or_default().push(entry);
        }
    }
    let items = by_name
        .into_iter()
        .filter_map(|(name, entries)| {
            let [entry] = entries.as_slice() else {
                return None;
            };
            let contract = entry.contract.as_ref()?;
            Some(CompletionItem {
                label: name.to_string(),
                kind: CompletionKind::Variable,
                detail: Some(format!(
                    "{} (trusted context, required)",
                    context_type_label(contract)
                )),
                documentation: None,
                insert_text: None,
            })
        })
        .collect();
    emit_completion_candidate(&mut commands, request, items);
}

async fn define_context_uses(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    uses: Query<
        (Entity, &ResolvedContextUse, &ContextDeclarationKey),
        Where<BowlEq<BelongsToFile>>,
    >,
    navigation: Query<
        (
            Entity,
            &ContextDeclarationNavigation,
            &ContextDeclarationKey,
        ),
        Where<BowlEq<ContextDeclarationKey>>,
    >,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, resolved, _) = uses.item();
    if !resolved.span.contains(cursor.0) {
        return;
    }
    if !matches!(resolved.resolution, ContextUseResolution::Resolved { .. }) {
        return;
    }
    let (_, navigation, _) = navigation.item();
    commands.entity(request).insert(DefinitionTarget::Source {
        file: navigation.file,
        span: navigation.name_span,
    });
}
