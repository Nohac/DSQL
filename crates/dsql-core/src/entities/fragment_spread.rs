//! Fragment-spread entity: `...Name` selections, their resolution to
//! fragment definitions, and the unknown-fragment check.

use bowl::{
    Bowl, Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, SystemExt, View, Where,
    With,
};

use crate::entities::definition::{DefDecl, DefIndex, DefKind, FragmentKey};
use crate::entities::{direct_token, node_span, text};
use crate::entity::{CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage};
use crate::format::CstFormatter;
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    ParentKey, Severity, Span, emit_diagnostic,
};
use crate::service::hover::{HoverCandidate, HoverEnriched, HoverFile, Position, priority, span_matches};
use crate::grammar::lexer::Token;
use crate::grammar::parser::NodeRef;

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
        bowl.add_system(resolve_spreads).await;
        bowl.add_system(check_unknown_fragments).await;
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

        match ctx.parent {
            Some(parent) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                ParentKey(parent),
                FragmentKey(name),
                decl,
            )),
            None => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                FragmentKey(name),
                decl,
            )),
        };
    }
}

/// Same name, same file: the bound-join filter matching a spread to the
/// fragment definitions it can refer to.
type SameNameSameFile = Where<bowl::And<BowlEq<FragmentKey>, BowlEq<BelongsToFile>>>;

/// Resolves each spread to the same-named fragment definition in the same
/// file — a bound join: one invocation per (spread, fragment) pair whose
/// `FragmentKey` and `BelongsToFile` both match.
async fn resolve_spreads(
    spreads: Query<(Entity, &SpreadDecl, &FragmentKey, &BelongsToFile)>,
    fragments: Query<(Entity, &DefDecl), SameNameSameFile>,
    mut commands: Commands,
) {
    let (spread, _, _, _) = spreads.item();
    let (fragment, _) = fragments.item();

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

    // Unknown fragments are reported by check_unknown_fragments.
    let Some((_, _, target, fragment_key)) = ctx.tree.fragment_named(&spread.name).copied() else {
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
        let Some((_, _, _, next_key)) = ctx.tree.fragment_named(&name).copied() else {
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

/// Reports spreads that name no fragment in their file. The tracked
/// [`DefIndex`] input reruns rows when the definition set changes; the
/// ambient `View` alone would never wake this check for an unrelated edit
/// that adds or removes the fragment being spread.
async fn check_unknown_fragments(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &SpreadDecl, &BelongsToFile)>,
    _index: Query<(Entity, &DefIndex)>,
    fragments: View<'_, (Entity, &DefDecl, &BelongsToFile)>,
    mut commands: Commands,
) {
    let (spread, decl, file) = query.item();

    let resolves = fragments.iter().any(|(_, fragment, fragment_file)| {
        fragment.kind == DefKind::Fragment
            && fragment.name == decl.name
            && fragment_file.0 == file.0
    });
    if resolves {
        return;
    }

    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(spread),
            file: file.0,
            span: decl.name_span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code: DiagnosticCode::UnknownFragment,
            message: format!("fragment `{}` not found", decl.name),
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

/// Answers hover on a `...Name` spread with the fragment it resolves to.
async fn hover_spreads(
    query: Query<(Entity, &HoverFile, &Position), With<HoverEnriched>>,
    spreads: View<'_, (Entity, &SpreadDecl, &BelongsToFile)>,
    targets: View<'_, (Entity, &crate::entities::definition::FragmentTarget, &BelongsToFile)>,
    mut commands: Commands,
) {
    let (request, file, position) = query.item();

    let Some((_, spread, _)) = spreads.iter().find(|(_, spread, spread_file)| {
        span_matches(spread.name_span, spread_file.0, file.0, position.offset)
    }) else {
        return;
    };

    let target = targets
        .iter()
        .find(|(_, _, target_file)| target_file.0 == file.0)
        .map(|(_, target, _)| target.name.clone());
    let text = match target {
        Some(target) => format!("fragment `{}` on `{target}`", spread.name),
        None => format!("fragment `{}`", spread.name),
    };

    commands.insert((
        DerivedFrom::new(request),
        HoverCandidate {
            request,
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
    fragments: View<'_, (Entity, &DefDecl, &crate::entities::definition::FragmentTarget)>,
    catalog: Query<(Entity, &crate::catalog::CatalogSnapshot)>,
    mut commands: Commands,
) {
    use crate::catalog::TableRef;
    use crate::service::completion::{CompletionCandidate, CompletionItem, CompletionKind, CompletionSite};

    let (request, context) = requests.item();
    let (_, snapshot) = catalog.item();

    let Some(table) = context.table else {
        return;
    };
    let spread_site = match context.site {
        CompletionSite::SpreadName => true,
        CompletionSite::SelectionBody => false,
        _ => return,
    };

    for (_, decl, target) in fragments.iter() {
        let Some(target_table) = snapshot.catalog().table_ref_for(TableRef::parse(&target.name))
        else {
            continue;
        };
        if target_table.id != table {
            continue;
        }
        commands.insert((
            DerivedFrom::new(request),
            CompletionCandidate {
                request,
                item: CompletionItem {
                    label: decl.name.clone(),
                    kind: CompletionKind::Fragment,
                    detail: Some(format!("fragment on {}", target.name)),
                    insert_text: (!spread_site).then(|| format!("...{}", decl.name)),
                },
            },
        ));
    }
}
