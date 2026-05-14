use crate::{
    catalog::TableKey,
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
        candidates: Vec<TableKey>,
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
}

impl CheckError {
    pub fn to_diagnostic(&self) -> Diagnostic {
        let (code, message) = match &self.kind {
            CheckErrorKind::DuplicateFragment { name } => (
                DiagnosticCode::DuplicateDefinition,
                format!("duplicate fragment `{name}`"),
            ),
            CheckErrorKind::TableNotFound { table } => (
                DiagnosticCode::TableNotFound,
                format!("table `{table}` not found"),
            ),
            CheckErrorKind::AmbiguousTable { table, candidates } => (
                DiagnosticCode::AmbiguousTable,
                format!(
                    "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    table,
                    format_table_candidates(candidates)
                ),
            ),
            CheckErrorKind::FieldNotFound { field, table } => (
                DiagnosticCode::FieldNotFound,
                format!("field `{field}` not found on table `{table}`"),
            ),
            CheckErrorKind::AmbiguousRelation {
                relation,
                candidates,
            } => (
                DiagnosticCode::AmbiguousRelation,
                format!(
                    "relation `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    relation,
                    format_table_candidates(candidates)
                ),
            ),
            CheckErrorKind::DuplicateOutputKey { key } => (
                DiagnosticCode::DuplicateOutputKey,
                format!("selection output key `{key}` is ambiguous; use an alias"),
            ),
            CheckErrorKind::ScalarSelectionSet { field, data_type } => (
                DiagnosticCode::ScalarSelectionSet,
                format!(
                    "field `{field}` is a scalar ({data_type}) and cannot have a selection set"
                ),
            ),
            CheckErrorKind::ScalarClauses { field, data_type } => (
                DiagnosticCode::ScalarClauses,
                format!(
                    "field `{field}` is a scalar ({data_type}); only relations can have clauses"
                ),
            ),
            CheckErrorKind::RelationSelectionSet { field } => (
                DiagnosticCode::RelationSelectionSet,
                format!("relation field `{field}` must have a selection set"),
            ),
            CheckErrorKind::UnknownFragment { fragment } => (
                DiagnosticCode::UnknownFragment,
                format!("fragment `{fragment}` not found"),
            ),
            CheckErrorKind::FragmentTypeMismatch {
                fragment,
                expected,
                actual,
            } => (
                DiagnosticCode::FragmentTypeMismatch,
                format!(
                    "fragment `{fragment}` applies to `{actual}` and cannot be spread in `{expected}`"
                ),
            ),
            CheckErrorKind::CircularFragmentSpread { fragment } => (
                DiagnosticCode::UnknownFragment,
                format!("fragment `{fragment}` recursively spreads itself"),
            ),
            CheckErrorKind::ClauseValueTypeMismatch { clause, expected } => (
                DiagnosticCode::ClauseValueTypeMismatch,
                format!("clause `{clause}` expects {expected}"),
            ),
        };
        Diagnostic {
            range: self.range,
            severity: Severity::Error,
            code,
            message,
            source: DiagnosticSource::Check,
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
