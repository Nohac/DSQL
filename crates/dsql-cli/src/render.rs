//! Boundary rendering of diagnostic facts with miette.
//!
//! Diagnostics live in the bowl as pure facts — span, severity, code,
//! message, owning file. Rendering them with source excerpts is boundary
//! work: this module scoops the facts, resolves extracted regions back to
//! their host path and coordinates, pairs each finding with its source
//! text, and hands miette the result.

use std::io::IsTerminal;

use bowl::{Bowl, Entity, Query};
use miette::{
    Diagnostic as MietteDiagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan,
    NamedSource, Severity as MietteSeverity, SourceCode,
};

use dsql_core::facts::{
    BelongsToFile, Diagnostic, DiagnosticCode, DiagnosticSource, Severity, Span,
};
use dsql_core::source::{BelongsToHost, FilePath, SourceOffset, SourceText};

/// One finding ready to render: message and identity from the fact,
/// span already in host coordinates, source text of the reported file.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SourceDiagnostic {
    message: String,
    code: String,
    severity: Severity,
    start: usize,
    end: usize,
    named_source: NamedSource<String>,
}

impl MietteDiagnostic for SourceDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(&self.code))
    }

    fn severity(&self) -> Option<MietteSeverity> {
        Some(match self.severity {
            Severity::Error => MietteSeverity::Error,
            Severity::Warning => MietteSeverity::Warning,
            Severity::Info => MietteSeverity::Advice,
        })
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.named_source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::underline(
            self.start..self.end,
        ))))
    }
}

impl SourceDiagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Scoops every diagnostic fact into renderable form, sorted by file and
/// span. Findings on extracted regions report under their host file, with
/// spans shifted into host coordinates; source text comes from the bowl,
/// so no file is re-read.
pub async fn collect_diagnostics(bowl: &Bowl) -> Vec<SourceDiagnostic> {
    type Row<'a> = (
        Entity,
        &'a Severity,
        &'a Span,
        &'a Diagnostic,
        &'a DiagnosticCode,
        &'a DiagnosticSource,
        &'a BelongsToFile,
    );
    let rows = bowl.scoop::<Query<Row<'_>>>().await;
    let paths = bowl
        .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
        .await;
    let paths = paths.collect();
    let regions = bowl
        .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
        .await;
    let regions = regions.collect();

    // A finding's file is a plain document or a region; regions render
    // against their host.
    let locate = |file: Entity| -> Option<(String, String, usize)> {
        let (target, offset) = regions
            .iter()
            .find(|(entity, _, _)| *entity == file)
            .map_or((file, 0), |(_, host, offset)| (host.0, offset.0));
        paths
            .iter()
            .find(|(entity, _, _)| *entity == target)
            .map(|(_, path, text)| (path.0.clone(), text.to_text(), offset))
    };

    let mut diagnostics: Vec<(String, usize, SourceDiagnostic)> = rows
        .collect()
        .into_iter()
        .filter_map(|(_, severity, span, diagnostic, code, source, file)| {
            let (path, text, offset) = locate(file.0)?;
            let start = offset + span.start;
            let end = offset + span.end;
            Some((
                path.clone(),
                start,
                SourceDiagnostic {
                    message: diagnostic.0.clone(),
                    code: format!("dsql::{source:?}::{code:?}"),
                    severity: *severity,
                    start,
                    end,
                    named_source: NamedSource::new(path, text),
                },
            ))
        })
        .collect();
    diagnostics.sort_by(|(left_path, left_start, _), (right_path, right_start, _)| {
        left_path.cmp(right_path).then(left_start.cmp(right_start))
    });
    diagnostics
        .into_iter()
        .map(|(_, _, diagnostic)| diagnostic)
        .collect()
}

/// Renders one finding, colored when stdout is a terminal.
pub fn render(diagnostic: &SourceDiagnostic) -> String {
    let theme = if std::io::stdout().is_terminal() {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    render_themed(diagnostic, theme)
}

pub fn render_themed(diagnostic: &SourceDiagnostic, theme: GraphicalTheme) -> String {
    let mut out = String::new();
    let handler = GraphicalReportHandler::new_themed(theme);
    if handler.render_report(&mut out, diagnostic).is_err() {
        // Rendering is presentation only; degrade to the raw message.
        out = format!("{}\n", diagnostic.message);
    }
    out
}
