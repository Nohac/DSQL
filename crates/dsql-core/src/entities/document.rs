//! Document entity: source files as they enter the bowl, and the parse that
//! turns their text into a lossless CST.

use crate::schema::{AstFacts, dsql_schema};
use bowl::{Commands, Component, DerivedFrom, Entity, MutRef, Query, Registrar, View, With};

use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    DiagnosticCode, DiagnosticFacts, DiagnosticSource, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{CstData, NodeRef, Parser};
use crate::source::{AnalysisResidency, DsqlDocument, OpenBuffer, SourceText};

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

    fn register(reg: &mut Registrar<'_>) {
        reg.system(parse_file);
    }
}

impl LowerStage for Document {
    // The document owns the file root, but everything under it is lowered
    // by the entities owning the definition rules — nothing to emit here.
    fn lower(
        _ctx: &LowerCtx<'_>,
        _node: NodeRef,
        _commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        None
    }
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

/// A document's text with what the eviction decision needs: mutable for
/// the post-parse evict, plus the editor-ownership marker exempting it.
pub(crate) type EvictableText<'a, Marker> =
    Query<(Entity, MutRef<'a, SourceText>, Option<&'a OpenBuffer>), With<Marker>>;

/// Parses each document's text into a [`ParsedFile`] and emits parse
/// errors as diagnostic entities owned by the file. Gated on
/// [`DsqlDocument`]: host sources carry text too, but only their
/// extracted regions parse.
///
/// This is the consumption boundary of the evictable-rope design: under
/// [`AnalysisResidency`], the rope is dropped once [`ParsedFile`] holds
/// the materialized source (a redundant second copy in batch mode) — a
/// fingerprint-neutral write, so nothing re-derives. Editor buffers
/// ([`OpenBuffer`]) always stay resident.
pub async fn parse_file(
    query: EvictableText<'_, DsqlDocument>,
    residency: View<'_, (Entity, &AnalysisResidency)>,
    mut commands: Commands<(dsql_schema::ParsedFile, dsql_schema::Diagnostic)>,
) {
    let (file, mut text, open_buffer) = query.item();

    // An evicted rope means this text (same fingerprint) already parsed.
    let Some(source) = text.to_text() else {
        return;
    };
    let mut parse_diagnostics = Vec::new();
    let cst = Parser::new(&source, &mut parse_diagnostics)
        .parse(&mut parse_diagnostics)
        .into_data();

    commands.entity(file).insert(ParsedFile {
        cst,
        source: source.clone(),
    });
    if residency.iter().next().is_some() && open_buffer.is_none() {
        text.evict();
    }

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

impl FormatStage for Document {
    /// Top-level layout: definitions and comments separated by blank lines.
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        use crate::grammar::lexer::Token;
        use crate::grammar::parser::Rule;

        let mut first = true;
        for child in formatter.children(node) {
            match (formatter.rule(child), formatter.token(child)) {
                (_, Some(Token::Comment)) => {
                    formatter.blank_between_definitions(&mut first);
                    formatter.write_node_text(child);
                }
                (
                    Some(
                        Rule::QueryDef
                        | Rule::FragmentDef
                        | Rule::FilterDef
                        | Rule::ConditionDef
                        | Rule::ContextDef,
                    ),
                    _,
                ) => {
                    formatter.blank_between_definitions(&mut first);
                    formatter.format_child(child);
                }
                _ => {}
            }
        }
    }
}
