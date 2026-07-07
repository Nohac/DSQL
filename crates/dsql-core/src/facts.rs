//! Cross-cutting facts shared by every language entity: spans, diagnostics,
//! file anchoring, and demand markers.

use bowl::{Commands, Component, DerivedFrom, Entity};

/// Byte range into the source text of one file. Positions (line/column)
/// are computed at protocol boundaries only.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl From<std::ops::Range<usize>> for Span {
    fn from(range: std::ops::Range<usize>) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

/// Severity of a [`Diagnostic`] fact.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub enum Severity {
    Warning,
    Error,
}

/// Human-readable diagnostic message. A diagnostic entity carries this plus
/// [`Severity`], [`Span`], [`BelongsToFile`], and `DerivedFrom` (ownership,
/// so stale diagnostics clean up when their source changes).
#[derive(Component, Hash, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct Diagnostic(pub String);

/// Join key anchoring a derived fact to the file entity it came from.
/// Lets services and adapters filter facts per file with bound queries.
#[derive(Component, Hash, Debug, Clone, Copy, PartialEq, Eq)]
#[component(hash)]
pub struct BelongsToFile(pub Entity);

/// Demand marker: diagnostics systems gate on this singleton fact, so a
/// bowl that nobody asked diagnostics from never plans them. The LSP
/// inserts it when the editor goes idle — debounce as data, not timers.
#[derive(Component, Hash)]
#[component(hash)]
pub struct DiagnosticsDemand;

/// Emits one diagnostic entity.
///
/// `derived_from` names the source facts whose change retires the
/// diagnostic; `file` anchors it to the file whose text `span` indexes.
pub fn emit_diagnostic(
    commands: &mut Commands,
    derived_from: DerivedFrom,
    file: Entity,
    span: Span,
    severity: Severity,
    message: impl Into<String>,
) {
    commands.insert((
        derived_from,
        BelongsToFile(file),
        span,
        severity,
        Diagnostic(message.into()),
    ));
}
