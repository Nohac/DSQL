//! Boundary rendering of diagnostic facts with miette.
//!
//! Diagnostics live in the bowl as pure facts — span, severity, code,
//! message, owning file. Rendering them with source excerpts is boundary
//! work: this module scoops the facts, resolves extracted regions back to
//! their host path and coordinates, and hands miette a rope-backed
//! source reader so only the excerpt lines ever materialize.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::{Arc, Mutex};

use bowl::{Bowl, Entity, Query};
use miette::{
    Diagnostic as MietteDiagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan,
    MietteError, MietteSpanContents, Severity as MietteSeverity, SourceCode, SourceSpan,
    SpanContents,
};
use ropey::{LineType, Rope};

use dsql_core::facts::{
    BelongsToFile, Diagnostic, DiagnosticCode, DiagnosticSource, Severity, Span,
};
use dsql_core::source::{BelongsToHost, FilePath, SourceOffset, SourceText};

/// A miette [`SourceCode`] reading straight from a rope: `read_span`
/// materializes only the requested context lines, so rendering never
/// copies whole files. Rope clones are cheap (chunk-shared), so one
/// reader per finding costs nothing.
#[derive(Clone, Debug)]
struct RopeSource {
    name: String,
    rope: Arc<Rope>,
    /// Excerpt buffers handed to miette; kept alive for `&self`'s
    /// lifetime because [`SpanContents`] borrows from the reader.
    buffers: Arc<Mutex<Vec<Box<[u8]>>>>,
}

impl RopeSource {
    fn new(name: String, rope: Arc<Rope>) -> Self {
        Self {
            name,
            rope,
            buffers: Arc::default(),
        }
    }

    fn cache_bytes(&self, bytes: Vec<u8>) -> &[u8] {
        let mut buffers = self
            .buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = buffers.len();
        buffers.push(bytes.into_boxed_slice());
        let cached = &buffers[index];
        let ptr = cached.as_ptr();
        let len = cached.len();
        drop(buffers);

        // SAFETY: the slice points into a boxed buffer stored in
        // `self.buffers`. Box allocations remain stable when the Vec
        // reallocates, and the returned span contents are tied to the
        // lifetime of `&self`, so the cache cannot be dropped while miette
        // can still read the slice.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl SourceCode for RopeSource {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> std::result::Result<Box<dyn SpanContents<'a> + 'a>, MietteError> {
        let line_type = LineType::LF_CR;
        let len = self.rope.len();
        let span_end = span
            .offset()
            .checked_add(span.len())
            .filter(|end| *end <= len)
            .ok_or(MietteError::OutOfBounds)?;
        if span.offset() > len {
            return Err(MietteError::OutOfBounds);
        }

        let start_line = self.rope.byte_to_line_idx(span.offset(), line_type);
        let end_byte = if span_end == span.offset() {
            span.offset()
        } else {
            span_end.saturating_sub(1)
        };
        let end_line = self.rope.byte_to_line_idx(end_byte, line_type);
        let context_start_line = start_line.saturating_sub(context_lines_before);
        let context_end_line = end_line
            .saturating_add(context_lines_after)
            .saturating_add(1)
            .min(self.rope.len_lines(line_type));
        let context_start = self.rope.line_to_byte_idx(context_start_line, line_type);
        let context_end = if context_end_line >= self.rope.len_lines(line_type) {
            len
        } else {
            self.rope.line_to_byte_idx(context_end_line, line_type)
        };
        let column = span
            .offset()
            .saturating_sub(self.rope.line_to_byte_idx(start_line, line_type));
        let line_count = context_end_line.saturating_sub(context_start_line).max(1);
        let data = self
            .rope
            .slice(context_start..context_end)
            .to_string()
            .into_bytes();
        let contents = MietteSpanContents::new_named(
            self.name.clone(),
            self.cache_bytes(data),
            (context_start, context_end.saturating_sub(context_start)).into(),
            context_start_line,
            column,
            line_count,
        )
        .with_language("dsql");
        Ok(Box::new(contents))
    }
}

/// One finding ready to render: message and identity from the fact, span
/// already in host coordinates, source read from the file's rope.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SourceDiagnostic {
    message: String,
    code: String,
    severity: Severity,
    start: usize,
    end: usize,
    rope_source: RopeSource,
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
        Some(&self.rope_source)
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
/// spans shifted into host coordinates; source comes from the bowl's
/// ropes, so no file is re-read and nothing materializes wholesale.
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
    // against their host. One shared rope per file across its findings.
    let mut ropes: HashMap<Entity, Arc<Rope>> = HashMap::new();
    let mut locate = |file: Entity| -> Option<(String, Arc<Rope>, usize)> {
        let (target, offset) = regions
            .iter()
            .find(|(entity, _, _)| *entity == file)
            .map_or((file, 0), |(_, host, offset)| (host.0, offset.0));
        let (_, path, text) = paths.iter().find(|(entity, _, _)| *entity == target)?;
        let rope = ropes
            .entry(target)
            .or_insert_with(|| Arc::new(text.rope().clone()))
            .clone();
        Some((path.0.clone(), rope, offset))
    };

    let mut diagnostics: Vec<(String, usize, SourceDiagnostic)> = rows
        .collect()
        .into_iter()
        .filter_map(|(_, severity, span, diagnostic, code, source, file)| {
            let (path, rope, offset) = locate(file.0)?;
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
                    rope_source: RopeSource::new(path, rope),
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
