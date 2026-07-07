//! Document entity: source files as they enter the bowl, and the parse that
//! turns their text into a lossless CST.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query};

use crate::entity::{LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    DiagnosticCode, DiagnosticFacts, DiagnosticSource, Severity, Span, emit_diagnostic,
};
use crate::grammar::parser::{CstData, NodeRef, Parser};
use crate::source::SourceText;

/// The parsed form of one file: the CST plus the exact source snapshot it
/// was parsed from, so byte spans extracted during lowering are always
/// valid regardless of later edits to [`SourceText`].
#[derive(Component)]
pub struct ParsedFile {
    pub cst: CstData,
    pub source: String,
}

/// Owns the `document` root rule and the parse itself.
pub struct Document;

impl LanguageEntity for Document {
    const NAME: &'static str = "document";

    async fn register(bowl: &Bowl) {
        bowl.add_system(parse_file).await;
    }
}

impl LowerStage for Document {
    // The document owns the file root, but everything under it is lowered
    // by the entities owning the definition rules — nothing to emit here.
    fn lower(_ctx: &LowerCtx<'_>, _node: NodeRef, _commands: &mut Commands) {}
}

/// Maps a parse diagnostic message onto its machine-readable code.
///
/// Lexer and parser diagnostics arrive through the generated parser as
/// message + span only, so this keys on the messages our vendored lelwel
/// emits — all three shapes are pinned by the parse snapshot tests.
fn parse_diagnostic_code(message: &str) -> DiagnosticCode {
    if message == "invalid token" {
        DiagnosticCode::InvalidToken
    } else if message.contains("<end of file>") {
        DiagnosticCode::UnexpectedEof
    } else {
        DiagnosticCode::UnexpectedToken
    }
}

/// Parses each file's text into a [`ParsedFile`] and emits parse errors as
/// diagnostic entities owned by the file.
pub async fn parse_file(query: Query<(Entity, &SourceText)>, mut commands: Commands) {
    let (file, text) = query.item();

    let source = text.to_text();
    let mut parse_diagnostics = Vec::new();
    let cst = Parser::new(&source, &mut parse_diagnostics)
        .parse(&mut parse_diagnostics)
        .into_data();

    // Insert ParsedFile BEFORE emitting diagnostics: DerivedFrom anchors
    // capture the source entity's revision in command-application order, and
    // this insert bumps the file's revision. Emitted the other way around,
    // the diagnostics are stale on arrival and cleanup reaps them.
    commands.entity(file).insert(ParsedFile { cst, source });

    for diagnostic in parse_diagnostics {
        let span = diagnostic
            .labels
            .first()
            .map(|label| Span::from(label.range.clone()))
            .unwrap_or(Span { start: 0, end: 0 });
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(file),
                file,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Parse,
                code: parse_diagnostic_code(&diagnostic.message),
                message: diagnostic.message,
            },
        );
    }
}
