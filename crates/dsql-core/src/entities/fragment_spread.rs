//! Fragment-spread entity: `...Name` selections, their resolution to
//! fragment definitions, and the unknown-fragment check.

use bowl::{
    Bowl, Commands, Component, DerivedFrom, Entity, Phase, Query, SystemExt, View, Where, With,
};

use crate::entities::definition::{DefDecl, DefIndex, DefKind, FragmentKey};
use crate::entities::{direct_token, node_span, text};
use crate::entity::{
    CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage,
};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    ParentKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::NodeRef;
use crate::service::hover::{HoverCandidate, HoverEnriched, Position, RequestKey, priority};
use crate::source::{ResolutionScope, ScopeImports};

/// One `...Name` spread, lowered from `fragment_spread`.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct SpreadDecl {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
}

/// Derived resolution fact: `spread` refers to fragment definition
/// `fragment` in the same file. Planning expands through these; editor
/// go-to-definition follows them.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct SpreadResolution {
    pub spread: Entity,
    pub fragment: Entity,
}

/// Owns `fragment_spread`.
pub struct FragmentSpread;

impl LanguageEntity for FragmentSpread {
    const NAME: &'static str = "fragment_spread";

    async fn register(bowl: &Bowl) {
        // Both view lowered fragment definitions ambiently, so they sit
        // behind the Complete phase barrier; their DefIndex/ScopeImports
        // inputs are tracked, which exempts them from the same-phase race
        // next to index_defs.
        bowl.add_system(resolve_spreads.run_during(Phase::Complete))
            .await;
        bowl.add_system(check_unknown_fragments.run_during(Phase::Complete))
            .await;
    }
}

impl LowerStage for FragmentSpread {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
        let Some(name_span) = direct_token(ctx.cst, node, Token::Name) else {
            // `...` without a name; parse diagnostics cover it.
            return;
        };

        let name = text(ctx.source, name_span).to_string();
        let decl = SpreadDecl {
            name: name.clone(),
            name_span,
            span: node_span(ctx.cst, node),
        };

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let scope = ResolutionScope(ctx.scope.to_string());
        let entity = commands.insert((
            DerivedFrom::new(ctx.file),
            BelongsToFile(ctx.file),
            key,
            scope,
            FragmentKey(name),
            decl,
        ));
        if let Some(parent) = ctx.parent {
            commands.entity(entity).insert(ParentKey(parent));
        }
    }
}

/// The views spread services resolve against, bundled for reuse.
#[derive(bowl::SystemParam)]
pub(crate) struct SpreadResolver<'a> {
    imports: View<'a, (Entity, &'a ScopeImports)>,
    fragments: View<'a, (Entity, &'a DefDecl, &'a ResolutionScope)>,
    targets: View<'a, (Entity, &'a crate::entities::definition::FragmentTarget)>,
}

impl SpreadResolver<'_> {
    /// The `on` target of the uniquely visible fragment `name` from
    /// `scope`, if any.
    pub(crate) fn target_of(&self, name: &str, scope: &str) -> Option<String> {
        let (_, imports) = self.imports.iter().next()?;
        let candidates = visible_fragments(name, scope, imports, self.fragments.iter());
        let [(fragment, _, _)] = candidates.as_slice() else {
            return None;
        };
        self.targets
            .iter()
            .find(|(entity, _)| entity == fragment)
            .map(|(_, target)| target.name.clone())
    }
}

/// The fragment definitions a spread in `scope` can see, per the effective
/// resolver (docs/spec/resolution-scopes.md): the scope's own fragments
/// plus its direct imports'. Shared by resolution, checks, planning,
/// variables, and services.
pub(crate) fn visible_fragments<'a>(
    name: &'a str,
    scope: &'a str,
    imports: &'a ScopeImports,
    fragments: impl IntoIterator<Item = (Entity, &'a DefDecl, &'a ResolutionScope)> + 'a,
) -> Vec<(Entity, &'a DefDecl, &'a ResolutionScope)> {
    fragments
        .into_iter()
        .filter(|(_, decl, fragment_scope)| {
            decl.kind == DefKind::Fragment
                && decl.name == name
                && imports
                    .visible_from(scope)
                    .any(|visible| visible == fragment_scope.0)
        })
        .collect()
}

/// Resolves each spread to the fragment its scope sees. Exactly one
/// visible candidate resolves; zero or several resolve nothing — the
/// unknown/ambiguity checks report those. The tracked [`DefIndex`] and
/// [`ScopeImports`] inputs rerun rows when the definition set or the scope
/// graph changes.
async fn resolve_spreads(
    spreads: Query<(Entity, &SpreadDecl, &ResolutionScope)>,
    _index: Query<(Entity, &DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    fragments: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    mut commands: Commands,
) {
    let (spread, decl, scope) = spreads.item();
    let (_, imports) = imports.item();

    let candidates = visible_fragments(&decl.name, &scope.0, imports, fragments.iter());
    let [(fragment, _, _)] = candidates.as_slice() else {
        return;
    };
    let fragment = *fragment;

    commands.insert((
        DerivedFrom::many([spread, fragment]),
        SpreadResolution { spread, fragment },
    ));
}

/// Checks one spread site during the selection check walk (see
/// `field_selection::check_selections`): the named fragment's target must
/// match the context table, and following spreads through fragments must
/// not cycle. Lives here because spread semantics belong to this entity;
/// the walk only orchestrates.
pub(crate) fn check_spread_site(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    spread_entity: Entity,
    spread: &SpreadDecl,
    context_table: crate::catalog::TableId,
) {
    use crate::catalog::TableRef;

    // Unknown and ambiguous fragments are reported by
    // check_unknown_fragments.
    let Some((_, _, target, fragment_key, _)) = ctx
        .tree
        .resolve_fragment(&spread.name, ctx.scope, ctx.imports)
        .copied()
    else {
        return;
    };

    let Some(target_table) = ctx.catalog.table_ref_for(TableRef::parse(&target.name)) else {
        // Unresolvable target is reported on the fragment definition.
        return;
    };
    let target_table_id = target_table.id;
    let target_table_name = target_table.name.clone();

    if target_table_id != context_table {
        let context_name = ctx
            .catalog
            .table_by_id(context_table)
            .map(|table| table.name.clone())
            .unwrap_or_default();
        ctx.error(
            spread_entity,
            spread.name_span,
            crate::facts::DiagnosticCode::FragmentTypeMismatch,
            format!(
                "fragment `{}` applies to `{target_table_name}` and cannot be spread in `{context_name}`",
                spread.name
            ),
        );
        return;
    }

    // Cycle detection: follow spreads through fragment bodies; a fragment
    // already on the path spreading again is a cycle.
    let mut path = vec![spread.name.clone()];
    detect_cycles(ctx, fragment_key, &mut path);
}

fn detect_cycles(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    fragment_key: crate::facts::NodeKey,
    path: &mut Vec<String>,
) {
    let inner_spreads = spreads_below(ctx, fragment_key);
    for (entity, name, name_span) in inner_spreads {
        if path.contains(&name) {
            ctx.error(
                entity,
                name_span,
                crate::facts::DiagnosticCode::CircularFragmentSpread,
                format!("fragment `{name}` recursively spreads itself"),
            );
            continue;
        }
        let Some((_, _, _, next_key, _)) = ctx
            .tree
            .resolve_fragment(&name, ctx.scope, ctx.imports)
            .copied()
        else {
            continue;
        };
        path.push(name);
        detect_cycles(ctx, next_key, path);
        path.pop();
    }
}

/// All spreads transitively below `parent` (through field selections, not
/// through fragment definitions).
fn spreads_below(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    parent: crate::facts::NodeKey,
) -> Vec<(Entity, String, Span)> {
    let mut found = Vec::new();
    for (entity, spread, _, _) in ctx.tree.spreads_under(parent) {
        found.push((*entity, spread.name.clone(), spread.name_span));
    }
    let children: Vec<crate::facts::NodeKey> = ctx
        .tree
        .fields_under(parent)
        .map(|(_, _, key, _)| *key)
        .collect();
    for child in children {
        found.extend(spreads_below(ctx, child));
    }
    found
}

/// Reports spreads that name no visible fragment, and spreads whose name
/// is provided by more than one visible scope. The tracked [`DefIndex`]
/// and [`ScopeImports`] inputs rerun rows when the definition set or the
/// scope graph changes; the ambient `View` alone would never wake this
/// check for an unrelated edit.
async fn check_unknown_fragments(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &SpreadDecl, &BelongsToFile, &ResolutionScope)>,
    _index: Query<(Entity, &DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    fragments: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    mut commands: Commands,
) {
    let (spread, decl, file, scope) = query.item();
    let (_, imports) = imports.item();

    let candidates = visible_fragments(&decl.name, &scope.0, imports, fragments.iter());
    let message = match candidates.as_slice() {
        [_] => return,
        [] => format!("fragment `{}` not found", decl.name),
        several => {
            let mut scopes: Vec<&str> = several
                .iter()
                .map(|(_, _, fragment_scope)| fragment_scope.0.as_str())
                .collect();
            scopes.sort();
            scopes.dedup();
            if scopes.len() == 1 {
                // Same-scope duplicates are the duplicate-definition
                // check's report; a second message here is noise.
                return;
            }
            format!(
                "fragment `{}` is ambiguous; provided by scopes {}",
                decl.name,
                scopes
                    .iter()
                    .map(|scope| format!("`{scope}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };

    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(spread),
            file: file.0,
            span: decl.name_span,
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

        if let Some(name) = formatter.direct_token_text(node, Token::Name) {
            formatter.write_str("...");
            formatter.write_str(&name);
        }
        for directive in formatter.direct_rules(node, Rule::Directive) {
            formatter.format_child(directive);
        }
    }
}

impl HoverStage for FragmentSpread {
    async fn register_hover(bowl: &Bowl) {
        bowl.add_system(hover_spreads.run_during(bowl::Phase::Complete))
            .await;
    }
}

/// Answers hover on a `...Name` spread with the fragment it resolves to:
/// one invocation per (request, spread-in-file) pair via the
/// `BelongsToFile` join. The fragment target still comes from the
/// cross-scope resolver views (scope visibility is not an equal-key join),
/// which is why this stays behind the Complete barrier.
async fn hover_spreads(
    query: Query<(Entity, &BelongsToFile, &Position), With<HoverEnriched>>,
    spreads: Query<(Entity, &SpreadDecl, &ResolutionScope), Where<bowl::Eq<BelongsToFile>>>,
    resolver: SpreadResolver<'_>,
    mut commands: Commands,
) {
    let (request, _file, position) = query.item();
    let (_, spread, scope) = spreads.item();

    if !(spread.name_span.start <= position.offset && position.offset < spread.name_span.end) {
        return;
    }

    let target = resolver.target_of(&spread.name, &scope.0);
    let text = match target {
        Some(target) => format!("fragment `{}` on `{target}`", spread.name),
        None => format!("fragment `{}`", spread.name),
    };

    commands.insert((
        DerivedFrom::new(request),
        RequestKey(request),
        HoverCandidate {
            priority: priority::SPREAD,
            text,
        },
    ));
}

impl CompletionStage for FragmentSpread {
    async fn register_completions(bowl: &Bowl) {
        bowl.add_system(complete_spreads.run_during(bowl::Phase::Complete))
            .await;
    }
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
    mut commands: Commands,
) {
    use crate::catalog::TableRef;
    use crate::service::completion::{
        CompletionCandidate, CompletionItem, CompletionKind, CompletionSite,
    };

    let (request, context) = requests.item();
    let (_, snapshot) = catalog.item();
    let (_, imports) = imports.item();

    let Some(table) = context.table else {
        return;
    };
    let spread_site = match context.site {
        CompletionSite::SpreadName => true,
        CompletionSite::SelectionBody => false,
        _ => return,
    };

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
        items.push(CompletionItem {
            label: decl.name.clone(),
            kind: CompletionKind::Fragment,
            detail: Some(format!("fragment on {}", target.name)),
            insert_text: (!spread_site).then(|| format!("...{}", decl.name)),
        });
    }
    if !items.is_empty() {
        commands.insert((
            DerivedFrom::new(request),
            RequestKey(request),
            CompletionCandidate { items },
        ));
    }
}
