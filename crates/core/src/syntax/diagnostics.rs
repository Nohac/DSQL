use super::TextRange;
use facet::Facet;

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
