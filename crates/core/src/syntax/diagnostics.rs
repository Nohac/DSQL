use super::TextRange;
use facet::Facet;
use miette::{LabeledSpan, SourceSpan};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub source: DiagnosticSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum DiagnosticCode {
    InvalidToken,
    UnexpectedToken,
    UnexpectedEof,
    DuplicateDefinition,
    UnknownFragment,
    FragmentTypeMismatch,
    TableNotFound,
    AmbiguousTable,
    FieldNotFound,
    AmbiguousRelation,
    DuplicateOutputKey,
    UnindexedJoinColumn,
    UnindexedScanColumn,
    ScalarSelectionSet,
    ScalarClauses,
    RelationSelectionSet,
    ClauseValueTypeMismatch,
    PredicateTypeMismatch,
    FormatParseError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum DiagnosticSource {
    Parse,
    Lower,
    Check,
    Lint,
    Plan,
    Format,
}

impl Diagnostic {
    pub fn parse_error(range: TextRange, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            range,
            severity: Severity::Error,
            code,
            message: message.into(),
            source: DiagnosticSource::Parse,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Diagnostic {}

impl miette::Diagnostic for Diagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("{:?}", self.code)))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
            Severity::Info => miette::Severity::Advice,
        })
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::underline(
            source_span(self.range),
        ))))
    }
}

pub(crate) fn source_span(range: TextRange) -> SourceSpan {
    (range.start as usize, (range.end - range.start) as usize).into()
}
