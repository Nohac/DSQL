//! Fragment-spread entity: `...Name` selections, their resolution to
//! fragment definitions, and the unknown-fragment check.

use bowl::{
    Commands, Component, DerivedFrom, Entity, Phase, Query, Registrar, SystemExt, View, Where, With,
};

use crate::entities::definition::{DefDecl, DefIndex, DefKind, FragmentKey};
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
#[derive(Component, Debug, Hash)]
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
    /// The uniquely visible fragment, when exactly one resolves.
    pub target: Option<SpreadTarget>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SpreadTarget {
    /// The fragment definition entity.
    pub fragment: Entity,
    /// The file holding the definition.
    pub file: Entity,
    /// The definition's name span within that file.
    pub name_span: crate::facts::Span,
    /// The fragment's `on` target name, when it has one.
    pub on: Option<String>,
}

/// Owns `fragment_spread`.
pub struct FragmentSpread;

impl LanguageEntity for FragmentSpread {
    const NAME: &'static str = "fragment_spread";

    fn register(reg: &mut Registrar<'_>) {
        // Both view lowered fragment definitions ambiently, so they sit
        // behind the Complete phase barrier; their DefIndex/ScopeImports
        // inputs are tracked, which exempts them from the same-phase race
        // next to index_defs.
        reg.system(resolve_spreads.run_during(Phase::Complete));
        reg.system(check_unknown_fragments.run_during(Phase::Complete));
        reg.system(hover_spreads);
        reg.system(complete_spreads.run_during(bowl::Phase::Complete));
    }
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

/// The fragment definitions a spread in `scope` can see, per the effective
/// resolver (docs/spec/resolution-scopes.md): the scope's own fragments
/// plus its transitive imports'. Shared by resolution, checks, planning,
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
    spreads: Query<(
        Entity,
        &SpreadDecl,
        &ResolutionScope,
        &crate::facts::BelongsToFile,
    )>,
    _index: Query<(Entity, &DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    fragments: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    files: View<'_, (Entity, &DefDecl, &crate::facts::BelongsToFile)>,
    targets: View<'_, (Entity, &crate::entities::definition::FragmentTarget)>,
    mut commands: Commands<(dsql_schema::ResolvedSpread,)>,
) {
    let (spread, decl, scope, file) = spreads.item();
    let (_, imports) = imports.item();

    let candidates = visible_fragments(&decl.name, &scope.0, imports, fragments.iter());
    let target = if let [(fragment, fragment_decl, _)] = candidates.as_slice() {
        let fragment = *fragment;
        files
            .iter()
            .find(|(entity, _, _)| *entity == fragment)
            .map(|(_, _, fragment_file)| SpreadTarget {
                fragment,
                file: fragment_file.0,
                name_span: fragment_decl.name_span,
                on: targets
                    .iter()
                    .find(|(entity, _)| *entity == fragment)
                    .map(|(_, target)| target.name.clone()),
            })
    } else {
        None
    };

    commands.insert((
        DerivedFrom::new(spread),
        crate::facts::BelongsToFile(file.0),
        ResolvedSpread {
            spread,
            name: decl.name.clone(),
            name_span: decl.name_span,
            target,
        },
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
    let Some((fragment_entity, _, target, _)) = ctx
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

    // Cycle detection: follow spreads through fragment bodies via the
    // shared expansion walker; a fragment already on the path spreading
    // again is a cycle.
    let mut expansion =
        crate::entities::expansion::SpreadExpansion::new(ctx.tree, ctx.scope, ctx.imports);
    let crate::entities::expansion::ExpandedSpread::Fragment { .. } = expansion.enter(&spread.name)
    else {
        return;
    };
    detect_cycles(ctx, &mut expansion, fragment_entity);
    expansion.leave();
}

fn detect_cycles(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    expansion: &mut crate::entities::expansion::SpreadExpansion<'_, '_>,
    fragment_entity: Entity,
) {
    use crate::entities::expansion::ExpandedSpread;

    let inner_spreads = spreads_below(ctx, fragment_entity);
    for (entity, name, name_span) in inner_spreads {
        match expansion.enter(&name) {
            ExpandedSpread::Cycle => {
                ctx.error(
                    entity,
                    name_span,
                    crate::facts::DiagnosticCode::CircularFragmentSpread,
                    format!("fragment `{name}` recursively spreads itself"),
                );
            }
            ExpandedSpread::Unresolved => {}
            ExpandedSpread::Fragment {
                entity: next_entity,
                ..
            } => {
                detect_cycles(ctx, expansion, next_entity);
                expansion.leave();
            }
        }
    }
}

/// All spreads transitively below `parent` (through field selections, not
/// through fragment definitions).
fn spreads_below(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    parent: Entity,
) -> Vec<(Entity, String, Span)> {
    let mut found = Vec::new();
    for (entity, spread, _) in ctx.tree.spreads_under(parent) {
        found.push((*entity, spread.name.clone(), spread.name_span));
    }
    let children: Vec<Entity> = ctx
        .tree
        .fields_under(parent)
        .map(|(entity, _, _, _)| *entity)
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
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
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

    let text = match resolved
        .target
        .as_ref()
        .and_then(|target| target.on.as_ref())
    {
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
