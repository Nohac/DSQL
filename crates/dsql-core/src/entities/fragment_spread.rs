//! Fragment-spread entity: `...Name` selections, their resolution to
//! fragment definitions, and the unknown-fragment check.

use bowl::{
    Commands, Component, DerivedFrom, Entity, Query, Registrar, Related, SystemExt, View, Where,
    With,
};

use crate::entities::definition::{DefDecl, FragmentKey, FragmentTarget};
use crate::entities::expansion::{
    SemanticDefinitionKey, SpreadResolutionOf, SpreadSiteGroup, SpreadSiteRoot,
};
use crate::entities::variable::VariableSource;
use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{Node, NodeRef, Rule};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::source::{ResolutionScope, ScopeImports};

/// One `...Name` spread, lowered from `fragment_spread`.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub struct SpreadDecl {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    /// Explicit public-input bindings, empty for default containment.
    pub bindings: Vec<SpreadBinding>,
}

/// One `$name`, `%name`, `$`, or `%` side of a spread binding.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SpreadBindingRef {
    pub source: VariableSource,
    pub name: Option<String>,
    pub span: Span,
}

/// One target binding and its optional explicit caller source.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SpreadBinding {
    pub target: SpreadBindingRef,
    /// `None` is named forwarding shorthand and therefore requires a name.
    pub source: Option<SpreadBindingRef>,
    pub span: Span,
}

/// Derived resolution fact for one spread: the fragment its scope
/// uniquely sees, with everything hover and go-to-definition need
/// denormalized in, so both stay tracked joins on [`BelongsToFile`]. A
/// separate entity, not a component on the spread — stamping syntax
/// entities would retire the diagnostics anchored to them.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedSpread {
    /// The spread entity this resolution is about.
    pub spread: Entity,
    /// The spread name (`...Name`).
    pub name: String,
    /// Span of the spread name in its document.
    pub name_span: crate::facts::Span,
    /// Scope-resolution outcome for this spread name.
    pub resolution: SpreadResolution,
}

impl ResolvedSpread {
    /// Returns the uniquely visible fragment target, if resolution succeeded.
    pub fn target(&self) -> Option<&SpreadTarget> {
        match &self.resolution {
            SpreadResolution::Resolved(target) => Some(target),
            SpreadResolution::Missing | SpreadResolution::NonUnique { .. } => None,
        }
    }
}

/// Scope-resolution outcome for one fragment spread.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SpreadResolution {
    /// Exactly one visible fragment supplies the name.
    Resolved(SpreadTarget),
    /// No visible fragment supplies the name.
    Missing,
    /// More than one visible fragment supplies the name.
    NonUnique {
        /// Distinct provider scopes in lexical order.
        provider_scopes: Vec<String>,
    },
}

/// Semantic identity of the fragment selected by one spread.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SpreadTarget {
    /// Candidate entity selected from the spread site's relationship.
    pub candidate: Entity,
    /// Stable key shared with the candidate's navigation payload.
    pub(crate) candidate_key: FragmentCandidateKey,
    /// Stable syntax key shared with the target fragment's semantic group.
    pub definition_key: SemanticDefinitionKey,
    /// The fragment's `on` target name, when it has one.
    pub on: Option<String>,
}

/// Semantic fragment candidate visible from one spread site.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub(crate) struct VisibleFragmentCandidate {
    pub(crate) definition_key: SemanticDefinitionKey,
    pub(crate) provider_scope: String,
    pub(crate) on: Option<String>,
}

/// Span-independent fragment identity projected onto its semantic group.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub(crate) struct FragmentSemantics {
    provider_scope: String,
    on: Option<String>,
}

/// Navigation payload for a [`VisibleFragmentCandidate`].
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub(crate) struct VisibleFragmentNavigation {
    pub(crate) file: Entity,
    pub(crate) name_span: Span,
}

/// Stable join key shared by one candidate and its navigation payload.
#[derive(Component, Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[component(hash)]
pub(crate) struct FragmentCandidateKey {
    spread: NodeKey,
    definition: SemanticDefinitionKey,
}

/// Relationship edge from a visible candidate to its spread site.
#[derive(Component, Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[component(untracked)]
#[relationship(target = VisibleFragmentCandidates)]
pub(crate) struct VisibleFragmentCandidateOf(pub(crate) Entity);

/// Engine-maintained visible fragment candidates for one spread site.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = VisibleFragmentCandidateOf)]
pub(crate) struct VisibleFragmentCandidates(pub(crate) Vec<Entity>);

/// Definition target for a uniquely resolved spread, kept separate from its
/// semantic resolution so span-only edits do not wake compiler consumers.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub(crate) struct ResolvedSpreadNavigation {
    pub(crate) file: Entity,
    pub(crate) name_span: Span,
}

type FragmentProjectionDefinition<'a> = Query<(
    Entity,
    &'a DefDecl,
    &'a FragmentKey,
    &'a ResolutionScope,
    Option<&'a FragmentTarget>,
    &'a SemanticDefinitionKey,
)>;
type CandidateSpread<'a> = Query<
    (
        Entity,
        &'a SpreadDecl,
        &'a FragmentKey,
        &'a ResolutionScope,
        &'a NodeKey,
    ),
    Where<bowl::Eq<FragmentKey>>,
>;
type NavigationFragment<'a> = Query<
    (
        Entity,
        &'a DefDecl,
        &'a ResolutionScope,
        &'a BelongsToFile,
        &'a SemanticDefinitionKey,
    ),
    Where<bowl::Eq<FragmentKey>>,
>;
type SpreadCandidateRows<'a> =
    Related<VisibleFragmentCandidates, (&'a VisibleFragmentCandidate, &'a FragmentCandidateKey)>;
type ResolutionSite<'a> = Query<(
    Entity,
    &'a SpreadSiteRoot,
    &'a NodeKey,
    SpreadCandidateRows<'a>,
)>;

fn scope_sees_provider(imports: &ScopeImports, consumer: &ResolutionScope, provider: &str) -> bool {
    imports
        .visible_from(&consumer.0)
        .any(|visible| visible == provider)
}

/// Owns `fragment_spread`.
pub struct FragmentSpread;

impl LanguageEntity for FragmentSpread {
    const NAME: &'static str = "fragment_spread";

    fn register(reg: &mut Registrar<'_>) {
        reg.system(project_fragment_semantics);
        reg.system(bind_visible_fragment_candidates);
        reg.system(bind_visible_fragment_navigation);
        reg.system(resolve_spreads);
        reg.system(resolve_spread_navigation);
        reg.system(check_unknown_fragments);
        reg.system(hover_spreads);
        // Completion enumerates candidates for an ephemeral request rather
        // than publishing persistent semantics, so its request-time view
        // remains behind the Complete barrier.
        reg.system(complete_spreads.run_during(bowl::Phase::Complete));
    }
}

/// Projects syntax-bearing fragment definitions into stable semantic rows.
/// Equal projection is a fingerprint cutoff for trivia and span edits.
async fn project_fragment_semantics(
    definitions: FragmentProjectionDefinition<'_>,
    mut commands: Commands<(dsql_schema::FragmentSemanticProjection,)>,
) {
    let (definition, _, fragment_key, scope, target, definition_key) = definitions.item();
    commands.insert((
        DerivedFrom::new(definition),
        fragment_key.clone(),
        *definition_key,
        FragmentSemantics {
            provider_scope: scope.0.clone(),
            on: target.map(|target| target.name.clone()),
        },
    ));
}

impl LowerStage for FragmentSpread {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let Some(name_span) = direct_name(ctx.cst, node) else {
            // `...` without a name; parse diagnostics cover it.
            return None;
        };

        let name = text(ctx.source, name_span).to_string();
        let bindings = direct_rule(ctx.cst, node, Rule::BindingList)
            .map(|list| build_bindings(ctx.cst, ctx.source, list))
            .unwrap_or_default();
        let decl = SpreadDecl {
            name: name.clone(),
            name_span,
            span: node_span(ctx.cst, node),
            bindings,
        };

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let scope = ResolutionScope(ctx.scope.to_string());
        let entity = match ctx.parent {
            Some(parent) => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    scope,
                    FragmentKey(name),
                    decl,
                    ChildOf(parent),
                ))
                .untyped(),
            None => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    scope,
                    FragmentKey(name),
                    decl,
                ))
                .untyped(),
        };
        commands.insert((DerivedFrom::new(entity), key, SpreadSiteRoot));
        Some(entity)
    }
}

fn build_bindings(
    cst: &crate::grammar::parser::CstData,
    source: &str,
    list: NodeRef,
) -> Vec<SpreadBinding> {
    cst.children(list)
        .filter(|child| cst.match_rule(*child, Rule::BindingItem))
        .filter_map(|item| {
            let mut refs = cst
                .children(item)
                .filter(|child| cst.match_rule(*child, Rule::BindingVariable))
                .filter_map(|variable| build_binding_ref(cst, source, variable));
            Some(SpreadBinding {
                target: refs.next()?,
                source: refs.next(),
                span: node_span(cst, item),
            })
        })
        .collect()
}

fn build_binding_ref(
    cst: &crate::grammar::parser::CstData,
    source: &str,
    variable: NodeRef,
) -> Option<SpreadBindingRef> {
    let source_kind = cst
        .children(variable)
        .find_map(|child| match cst.get(child) {
            Node::Token(Token::Dollar, _) => Some(VariableSource::Structured),
            Node::Token(Token::Percent, _) => Some(VariableSource::TopLevel),
            _ => None,
        })?;
    let name_span = direct_name(cst, variable);
    Some(SpreadBindingRef {
        source: source_kind,
        name: name_span.map(|span| text(source, span).to_string()),
        span: node_span(cst, variable),
    })
}

/// Materializes one visible same-name semantic candidate for one spread.
///
/// The semantic projection drives the first join. A provider edit reaches this
/// driver directly; a spread edit reaches it through the exact fragment-name
/// key, and the spread's [`NodeKey`] then binds its dedicated site.
async fn bind_visible_fragment_candidates(
    fragments: Query<(
        Entity,
        &FragmentKey,
        &FragmentSemantics,
        &SemanticDefinitionKey,
    )>,
    spreads: CandidateSpread<'_>,
    sites: Query<(Entity, &SpreadSiteRoot), Where<bowl::Eq<NodeKey>>>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::VisibleFragmentCandidate,)>,
) {
    let (fragment_projection, _, fragment, definition_key) = fragments.item();
    let (spread, _, _, spread_scope, spread_key) = spreads.item();
    let (site, _) = sites.item();
    let (_, imports) = imports.item();
    if !scope_sees_provider(imports, spread_scope, &fragment.provider_scope) {
        return;
    }

    let candidate_key = FragmentCandidateKey {
        spread: *spread_key,
        definition: *definition_key,
    };
    commands.insert((
        DerivedFrom::many([spread, fragment_projection]),
        VisibleFragmentCandidateOf(site),
        candidate_key,
        VisibleFragmentCandidate {
            definition_key: *definition_key,
            provider_scope: fragment.provider_scope.clone(),
            on: fragment.on.clone(),
        },
    ));
}

/// Materializes the source-location half of a visible candidate separately
/// from its semantic payload.
async fn bind_visible_fragment_navigation(
    spreads: Query<(
        Entity,
        &SpreadDecl,
        &FragmentKey,
        &ResolutionScope,
        &NodeKey,
    )>,
    fragments: NavigationFragment<'_>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::VisibleFragmentNavigation,)>,
) {
    let (spread, _, _, spread_scope, spread_key) = spreads.item();
    let (fragment, declaration, fragment_scope, file, definition_key) = fragments.item();
    let (_, imports) = imports.item();
    if !scope_sees_provider(imports, spread_scope, &fragment_scope.0) {
        return;
    }

    let candidate_key = FragmentCandidateKey {
        spread: *spread_key,
        definition: *definition_key,
    };
    commands.insert((
        DerivedFrom::many([spread, fragment]),
        candidate_key,
        VisibleFragmentNavigation {
            file: file.0,
            name_span: declaration.name_span,
        },
    ));
}

/// Resolves one spread from its exact relationship-owned candidate set.
///
/// A site can temporarily observe no candidates before same-phase candidate
/// binding converges. [`Related`] tracks that absence, so candidate insertion
/// reruns this site before any settled compiler result is published.
async fn resolve_spreads(
    sites: ResolutionSite<'_>,
    spreads: Query<
        (
            Entity,
            &SpreadDecl,
            &BelongsToFile,
            &crate::facts::SemanticMemberOf,
        ),
        Where<bowl::Eq<NodeKey>>,
    >,
    mut commands: Commands<(dsql_schema::ResolvedSpread,)>,
) {
    let (site, _, _, candidates) = sites.item();
    let (spread, declaration, file, semantic_group) = spreads.item();
    let mut candidates = candidates.iter().collect::<Vec<_>>();
    candidates.sort_by(|(left_entity, (left, _)), (right_entity, (right, _))| {
        left.provider_scope
            .cmp(&right.provider_scope)
            .then_with(|| left_entity.cmp(right_entity))
    });

    let resolution = match candidates.as_slice() {
        [] => SpreadResolution::Missing,
        [(candidate, (target, candidate_key))] => SpreadResolution::Resolved(SpreadTarget {
            candidate: *candidate,
            candidate_key: **candidate_key,
            definition_key: target.definition_key,
            on: target.on.clone(),
        }),
        _ => {
            let mut provider_scopes = candidates
                .iter()
                .map(|(_, (candidate, _))| candidate.provider_scope.clone())
                .collect::<Vec<_>>();
            provider_scopes.sort();
            provider_scopes.dedup();
            SpreadResolution::NonUnique { provider_scopes }
        }
    };
    let target = match &resolution {
        SpreadResolution::Resolved(target) => Some(target.clone()),
        SpreadResolution::Missing | SpreadResolution::NonUnique { .. } => None,
    };
    let resolved = commands.insert((
        DerivedFrom::new(spread),
        BelongsToFile(file.0),
        SpreadResolutionOf(semantic_group.0),
        SpreadSiteGroup(site),
        ResolvedSpread {
            spread,
            name: declaration.name.clone(),
            name_span: declaration.name_span,
            resolution,
        },
    ));
    if let Some(target) = target {
        commands.entity(resolved).insert(target.definition_key);
        commands.entity(resolved).insert(target.candidate_key);
    }
}

/// Projects definition navigation for a uniquely resolved spread without
/// coupling semantic consumers to definition spans.
async fn resolve_spread_navigation(
    resolutions: Query<(
        Entity,
        &ResolvedSpread,
        &FragmentCandidateKey,
        &BelongsToFile,
    )>,
    candidates: Query<
        (Entity, &VisibleFragmentNavigation, &FragmentCandidateKey),
        Where<bowl::Eq<FragmentCandidateKey>>,
    >,
    mut commands: Commands<(dsql_schema::ResolvedSpread,)>,
) {
    let (resolution, _, _, _) = resolutions.item();
    let (_, navigation, _) = candidates.item();
    commands
        .entity(resolution)
        .insert(ResolvedSpreadNavigation {
            file: navigation.file,
            name_span: navigation.name_span,
        });
}

/// Reports missing and cross-scope ambiguous fragment spreads from the one
/// semantic resolution outcome.
async fn check_unknown_fragments(
    _: Query<Entity, With<DiagnosticsDemand>>,
    resolutions: Query<(Entity, &ResolvedSpread, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (resolution, resolved, file) = resolutions.item();
    let message = match &resolved.resolution {
        SpreadResolution::Resolved(_) => return,
        SpreadResolution::Missing => format!("fragment `{}` not found", resolved.name),
        SpreadResolution::NonUnique { provider_scopes } if provider_scopes.len() == 1 => {
            // Same-scope duplicates are the duplicate-definition check's
            // report; a second message here is noise.
            return;
        }
        SpreadResolution::NonUnique { provider_scopes } => format!(
            "fragment `{}` is ambiguous; provided by scopes {}",
            resolved.name,
            provider_scopes
                .iter()
                .map(|scope| format!("`{scope}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(resolution),
            file: file.0,
            span: resolved.name_span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code: DiagnosticCode::UnknownFragment,
            message,
        },
    );
}

impl FormatStage for FragmentSpread {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        use crate::grammar::parser::Rule;

        if let Some(name) = formatter.direct_name_text(node) {
            formatter.write_str("...");
            formatter.write_str(&name);
        }
        if let Some(bindings) = formatter.direct_rule(node, Rule::BindingList) {
            formatter.binding_list(bindings);
        }
        for directive in formatter.direct_rules(node, Rule::Directive) {
            formatter.format_child(directive);
        }
    }
}

/// Answers hover on a `...Name` spread with the fragment it resolves to:
/// one tracked invocation per (request, spread-in-file) pair via the
/// `BelongsToFile` join, the target read off the [`ResolvedSpread`]
/// stamp — no views, no phase barrier.
async fn hover_spreads(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    spreads: Query<(Entity, &ResolvedSpread), Where<bowl::Eq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved) = spreads.item();

    if !resolved.name_span.contains(cursor.0) {
        return;
    }

    let text = match resolved.target().and_then(|target| target.on.as_ref()) {
        Some(target) => format!("fragment `{}` on `{target}`", resolved.name),
        None => format!("fragment `{}`", resolved.name),
    };

    emit_hover_candidate(&mut commands, request, priority::SPREAD, text);
}

/// Contributes fragments whose target matches the context table, both on a
/// partial `...Name` and as `...Name` insertions inside selection bodies.
async fn complete_spreads(
    requests: Query<
        (Entity, &crate::service::completion::CompletionContext),
        With<crate::service::completion::CompletionRequest>,
    >,
    fragments: View<
        '_,
        (
            Entity,
            &DefDecl,
            &crate::entities::definition::FragmentTarget,
            &ResolutionScope,
        ),
    >,
    imports: Query<(Entity, &ScopeImports)>,
    catalog: Query<(Entity, &crate::catalog::CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    use crate::catalog::TableRef;
    use crate::service::completion::{
        CompletionItem, CompletionKind, CompletionSite, emit_completion_candidate,
    };

    let (request, context) = requests.item();
    let (_, snapshot) = catalog.item();
    let (_, imports) = imports.item();

    let Some(table) = context.table else {
        return;
    };
    if !matches!(
        context.site,
        CompletionSite::SpreadName | CompletionSite::SelectionBody
    ) {
        return;
    }

    let mut items = Vec::new();
    for (_, decl, target, fragment_scope) in fragments.iter() {
        if !imports
            .visible_from(&context.scope)
            .any(|visible| visible == fragment_scope.0)
        {
            continue;
        }
        let Some(target_table) = snapshot
            .catalog()
            .table_ref_for(TableRef::parse(&target.name))
        else {
            continue;
        };
        if target_table.id != table {
            continue;
        }
        // A partial spread keeps the dots already typed: only the missing
        // ones are inserted before the name (none after a full `...`).
        let missing_dots = 3 - context.spread_dots;
        items.push(CompletionItem {
            label: decl.name.clone(),
            kind: CompletionKind::Fragment,
            detail: Some(format!("fragment on {}", target.name)),
            documentation: None,
            insert_text: (missing_dots > 0)
                .then(|| format!("{}{}", ".".repeat(missing_dots), decl.name)),
        });
    }
    emit_completion_candidate(&mut commands, request, items);
}
