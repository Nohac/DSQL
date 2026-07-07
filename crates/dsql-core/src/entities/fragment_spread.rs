//! Fragment-spread entity: `...Name` selections, their resolution to
//! fragment definitions, and the unknown-fragment check.

use bowl::{
    Bowl, Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, View, Where, With,
};

use crate::entities::definition::{DefDecl, DefIndex, DefKind, FragmentKey};
use crate::entities::{direct_token, node_span, text};
use crate::entity::{LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    ParentKey, Severity, Span, emit_diagnostic,
};
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

    async fn register(db: &Bowl) {
        db.add_system(resolve_spreads).await;
        db.add_system(check_unknown_fragments).await;
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
