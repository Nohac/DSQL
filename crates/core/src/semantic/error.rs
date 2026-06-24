use crate::{
    catalog::{DataType, LiteralKind, TableKey},
    diagnostics::{
        CompilerDiagnostic, CompilerDiagnosticSource, DsqlDiagnostic, extend_compiler_diagnostics,
    },
    syntax::{DiagnosticCode, DiagnosticSource, Severity, TextRange, source_span},
};
use facet::Facet;
use miette::LabeledSpan;
use std::fmt;

#[derive(Clone, Debug, Facet)]
pub struct CheckedFile {
    pub errors: Vec<CheckDiagnostic>,
    pub diagnostics: Vec<CheckDiagnostic>,
}

impl CompilerDiagnosticSource for CheckedFile {
    fn extend_compiler_diagnostics(&self, diagnostics: &mut Vec<CompilerDiagnostic>) {
        extend_compiler_diagnostics(diagnostics, self.diagnostics.iter().cloned());
    }
}

pub type CheckedDefinition = CheckedFile;
pub type CheckError = CheckDiagnostic;
pub type CheckErrorKind = CheckDiagnosticKind;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct CheckDiagnostic {
    pub range: TextRange,
    pub kind: CheckDiagnosticKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet, thiserror::Error)]
#[repr(C)]
pub enum CheckDiagnosticKind {
    #[error("duplicate fragment `{name}`")]
    DuplicateFragment { name: String },
    #[error("table `{table}` not found")]
    TableNotFound { table: String },
    #[error(
        "table `{table}` is ambiguous; use an alias with a schema-qualified name ({})",
        format_table_candidates(candidates)
    )]
    AmbiguousTable {
        table: String,
        candidates: Vec<TableKey>,
    },
    #[error("field `{field}` not found on table `{table}`")]
    FieldNotFound { field: String, table: String },
    #[error("relation `{relation}` has multiple foreign-key paths; use one of: {}", candidates.join(", "))]
    AmbiguousRelation {
        relation: String,
        candidates: Vec<String>,
    },
    #[error("selection output key `{key}` is ambiguous; use an alias")]
    DuplicateOutputKey { key: String },
    #[error(
        "selection output key `{key}` is {bytes} bytes; PostgreSQL result aliases must be at most {max} bytes"
    )]
    OutputKeyTooLong {
        key: String,
        bytes: usize,
        max: usize,
    },
    #[error("field `{field}` is a scalar ({data_type}) and cannot have a selection set")]
    ScalarSelectionSet { field: String, data_type: String },
    #[error("field `{field}` is a scalar ({data_type}); only relations can have clauses")]
    ScalarClauses { field: String, data_type: String },
    #[error("relation field `{field}` must have a selection set")]
    RelationSelectionSet { field: String },
    #[error("fragment `{fragment}` not found")]
    UnknownFragment { fragment: String },
    #[error("fragment `{fragment}` applies to `{actual}` and cannot be spread in `{expected}`")]
    FragmentTypeMismatch {
        fragment: String,
        expected: String,
        actual: String,
    },
    #[error("fragment `{fragment}` recursively spreads itself")]
    CircularFragmentSpread { fragment: String },
    #[error("clause `{clause}` expects {expected}")]
    ClauseValueTypeMismatch { clause: String, expected: String },
    #[error("field `{field}` expects {} but predicate uses {}", expected.expected_literal_description(), actual.as_str())]
    PredicateTypeMismatch {
        field: String,
        expected: DataType,
        actual: LiteralKind,
    },
    /// Directive name did not resolve to a known built-in or registered external directive.
    #[error("directive `{name}` not found")]
    UnknownDirective { name: String },
    /// Directive resolved, but cannot be used at the current semantic location.
    #[error("directive `{name}` is not allowed on {location}")]
    DirectiveNotAllowed { name: String, location: String },
    /// Directive invocation omitted a required argument.
    #[error("directive `{name}` requires argument `{argument}`")]
    MissingDirectiveArgument { name: String, argument: String },
    /// Directive invocation supplied an argument not declared by the directive.
    #[error("directive `{name}` does not define argument `{argument}`")]
    UnknownDirectiveArgument { name: String, argument: String },
    /// Directive invocation supplied the same argument more than once.
    #[error("directive `{name}` repeats argument `{argument}`")]
    DuplicateDirectiveArgument { name: String, argument: String },
    /// Directive argument value does not match the lightweight built-in expectation.
    #[error("directive `{name}` argument `{argument}` expects {expected}")]
    DirectiveArgumentTypeMismatch {
        name: String,
        argument: String,
        expected: String,
    },
}

impl CheckDiagnostic {
    pub fn to_diagnostic(&self) -> crate::Diagnostic {
        self.to_transport()
    }
}

impl CheckDiagnosticKind {
    pub fn code(&self) -> DiagnosticCode {
        match self {
            CheckDiagnosticKind::DuplicateFragment { .. } => DiagnosticCode::DuplicateDefinition,
            CheckDiagnosticKind::TableNotFound { .. } => DiagnosticCode::TableNotFound,
            CheckDiagnosticKind::AmbiguousTable { .. } => DiagnosticCode::AmbiguousTable,
            CheckDiagnosticKind::FieldNotFound { .. } => DiagnosticCode::FieldNotFound,
            CheckDiagnosticKind::AmbiguousRelation { .. } => DiagnosticCode::AmbiguousRelation,
            CheckDiagnosticKind::DuplicateOutputKey { .. } => DiagnosticCode::DuplicateOutputKey,
            CheckDiagnosticKind::OutputKeyTooLong { .. } => DiagnosticCode::OutputKeyTooLong,
            CheckDiagnosticKind::ScalarSelectionSet { .. } => DiagnosticCode::ScalarSelectionSet,
            CheckDiagnosticKind::ScalarClauses { .. } => DiagnosticCode::ScalarClauses,
            CheckDiagnosticKind::RelationSelectionSet { .. } => {
                DiagnosticCode::RelationSelectionSet
            }
            CheckDiagnosticKind::UnknownFragment { .. } => DiagnosticCode::UnknownFragment,
            CheckDiagnosticKind::FragmentTypeMismatch { .. } => {
                DiagnosticCode::FragmentTypeMismatch
            }
            CheckDiagnosticKind::CircularFragmentSpread { .. } => DiagnosticCode::UnknownFragment,
            CheckDiagnosticKind::ClauseValueTypeMismatch { .. } => {
                DiagnosticCode::ClauseValueTypeMismatch
            }
            CheckDiagnosticKind::PredicateTypeMismatch { .. } => {
                DiagnosticCode::PredicateTypeMismatch
            }
            CheckDiagnosticKind::UnknownDirective { .. } => DiagnosticCode::UnknownDirective,
            CheckDiagnosticKind::DirectiveNotAllowed { .. } => DiagnosticCode::DirectiveNotAllowed,
            CheckDiagnosticKind::MissingDirectiveArgument { .. } => {
                DiagnosticCode::MissingDirectiveArgument
            }
            CheckDiagnosticKind::UnknownDirectiveArgument { .. } => {
                DiagnosticCode::UnknownDirectiveArgument
            }
            CheckDiagnosticKind::DuplicateDirectiveArgument { .. } => {
                DiagnosticCode::DuplicateDirectiveArgument
            }
            CheckDiagnosticKind::DirectiveArgumentTypeMismatch { .. } => {
                DiagnosticCode::DirectiveArgumentTypeMismatch
            }
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for CheckDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for CheckDiagnostic {}

impl miette::Diagnostic for CheckDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("{:?}", self.kind.code())))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Error)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::underline(
            source_span(self.range),
        ))))
    }
}

impl DsqlDiagnostic for CheckDiagnostic {
    fn range(&self) -> TextRange {
        self.range
    }

    fn severity(&self) -> Severity {
        Severity::Error
    }

    fn code(&self) -> DiagnosticCode {
        self.kind.code()
    }

    fn source(&self) -> DiagnosticSource {
        DiagnosticSource::Check
    }
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}
