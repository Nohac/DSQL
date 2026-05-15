use crate::{
    catalog::{DataType, LiteralKind, TableKey},
    syntax::{Diagnostic, DiagnosticCode, DiagnosticSource, Severity, TextRange},
};
use facet::Facet;

#[derive(Clone, Debug, Facet)]
pub struct CheckedFile {
    pub errors: Vec<CheckError>,
    pub diagnostics: Vec<Diagnostic>,
}

pub type CheckedDefinition = CheckedFile;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct CheckError {
    pub range: TextRange,
    pub kind: CheckErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum CheckErrorKind {
    DuplicateFragment {
        name: String,
    },
    TableNotFound {
        table: String,
    },
    AmbiguousTable {
        table: String,
        candidates: Vec<TableKey>,
    },
    FieldNotFound {
        field: String,
        table: String,
    },
    AmbiguousRelation {
        relation: String,
        candidates: Vec<String>,
    },
    DuplicateOutputKey {
        key: String,
    },
    ScalarSelectionSet {
        field: String,
        data_type: String,
    },
    ScalarClauses {
        field: String,
        data_type: String,
    },
    RelationSelectionSet {
        field: String,
    },
    UnknownFragment {
        fragment: String,
    },
    FragmentTypeMismatch {
        fragment: String,
        expected: String,
        actual: String,
    },
    CircularFragmentSpread {
        fragment: String,
    },
    ClauseValueTypeMismatch {
        clause: String,
        expected: String,
    },
    PredicateTypeMismatch {
        field: String,
        expected: DataType,
        actual: LiteralKind,
    },
}

impl CheckError {
    pub fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic {
            range: self.range,
            severity: Severity::Error,
            code: self.kind.code(),
            message: self.kind.message(),
            source: DiagnosticSource::Check,
        }
    }
}

impl CheckErrorKind {
    pub fn code(&self) -> DiagnosticCode {
        match self {
            CheckErrorKind::DuplicateFragment { .. } => DiagnosticCode::DuplicateDefinition,
            CheckErrorKind::TableNotFound { .. } => DiagnosticCode::TableNotFound,
            CheckErrorKind::AmbiguousTable { .. } => DiagnosticCode::AmbiguousTable,
            CheckErrorKind::FieldNotFound { .. } => DiagnosticCode::FieldNotFound,
            CheckErrorKind::AmbiguousRelation { .. } => DiagnosticCode::AmbiguousRelation,
            CheckErrorKind::DuplicateOutputKey { .. } => DiagnosticCode::DuplicateOutputKey,
            CheckErrorKind::ScalarSelectionSet { .. } => DiagnosticCode::ScalarSelectionSet,
            CheckErrorKind::ScalarClauses { .. } => DiagnosticCode::ScalarClauses,
            CheckErrorKind::RelationSelectionSet { .. } => DiagnosticCode::RelationSelectionSet,
            CheckErrorKind::UnknownFragment { .. } => DiagnosticCode::UnknownFragment,
            CheckErrorKind::FragmentTypeMismatch { .. } => DiagnosticCode::FragmentTypeMismatch,
            CheckErrorKind::CircularFragmentSpread { .. } => DiagnosticCode::UnknownFragment,
            CheckErrorKind::ClauseValueTypeMismatch { .. } => {
                DiagnosticCode::ClauseValueTypeMismatch
            }
            CheckErrorKind::PredicateTypeMismatch { .. } => DiagnosticCode::PredicateTypeMismatch,
        }
    }

    pub fn message(&self) -> String {
        match self {
            CheckErrorKind::DuplicateFragment { name } => format!("duplicate fragment `{name}`"),
            CheckErrorKind::TableNotFound { table } => format!("table `{table}` not found"),
            CheckErrorKind::AmbiguousTable { table, candidates } => format!(
                "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                table,
                format_table_candidates(candidates),
            ),
            CheckErrorKind::FieldNotFound { field, table } => {
                format!("field `{field}` not found on table `{table}`")
            }
            CheckErrorKind::AmbiguousRelation {
                relation,
                candidates,
            } => format!(
                "relation `{}` has multiple foreign-key paths; use one of: {}",
                relation,
                candidates.join(", "),
            ),
            CheckErrorKind::DuplicateOutputKey { key } => {
                format!("selection output key `{key}` is ambiguous; use an alias")
            }
            CheckErrorKind::ScalarSelectionSet { field, data_type } => {
                format!("field `{field}` is a scalar ({data_type}) and cannot have a selection set",)
            }
            CheckErrorKind::ScalarClauses { field, data_type } => format!(
                "field `{field}` is a scalar ({data_type}); only relations can have clauses",
            ),
            CheckErrorKind::RelationSelectionSet { field } => {
                format!("relation field `{field}` must have a selection set")
            }
            CheckErrorKind::UnknownFragment { fragment } => {
                format!("fragment `{fragment}` not found")
            }
            CheckErrorKind::FragmentTypeMismatch {
                fragment,
                expected,
                actual,
            } => format!(
                "fragment `{fragment}` applies to `{actual}` and cannot be spread in `{expected}`",
            ),
            CheckErrorKind::CircularFragmentSpread { fragment } => {
                format!("fragment `{fragment}` recursively spreads itself")
            }
            CheckErrorKind::ClauseValueTypeMismatch { clause, expected } => {
                format!("clause `{clause}` expects {expected}")
            }
            CheckErrorKind::PredicateTypeMismatch {
                field,
                expected,
                actual,
            } => format!(
                "field `{field}` expects {} but predicate uses {}",
                expected.expected_literal_description(),
                actual.as_str(),
            ),
        }
    }
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}
