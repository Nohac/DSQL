use crate::{
    catalog::{ColumnId, ForeignKeyId, TableId, TableKey},
    diagnostics::DsqlDiagnostic,
    syntax::{BinaryOp, DiagnosticCode, DiagnosticSource, Severity, TextRange, source_span},
};
use facet::Facet;
use miette::LabeledSpan;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct PlannedFile {
    pub queries: Vec<QueryPlan>,
    pub diagnostics: Vec<PlanDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet, thiserror::Error)]
#[repr(C)]
pub enum PlanDiagnosticKind {
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
    #[error("fragment `{fragment}` not found")]
    UnknownFragment { fragment: String },
    #[error("relation `{relation}` has multiple foreign-key paths; use one of: {}", candidates.join(", "))]
    AmbiguousRelation {
        relation: String,
        candidates: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct PlanDiagnostic {
    pub range: TextRange,
    pub kind: PlanDiagnosticKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct QueryPlan {
    pub root: TableId,
    pub output_name: String,
    pub clauses: SelectionClauses,
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct FragmentPlan {
    pub table: TableId,
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SelectionPlan {
    pub table: TableId,
    pub clauses: SelectionClauses,
    pub items: Vec<SelectionPlanItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Facet)]
pub struct SelectionClauses {
    pub filter: Option<FilterExpr>,
    pub order_by: Vec<OrderByPlan>,
    pub limit: Option<SqlValue>,
    pub offset: Option<SqlValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct OrderByPlan {
    pub column: ColumnId,
    pub direction: SortDirectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SortDirectionPlan {
    Asc,
    Desc,
    Variant {
        path: String,
        variants: Vec<SqlVariantCase>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum FilterExpr {
    Column {
        scope: FilterColumnScope,
        column: ColumnId,
    },
    Literal(FilterLiteral),
    Parameter(SqlParameter),
    Binary {
        left: Box<FilterExpr>,
        op: BinaryOp,
        right: Box<FilterExpr>,
    },
    VariantBinary {
        left: Box<FilterExpr>,
        path: String,
        variants: Vec<SqlVariantCase>,
        right: Box<FilterExpr>,
    },
    Exists {
        foreign_key: ForeignKeyId,
        table: TableId,
        filter: Box<FilterExpr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum FilterColumnScope {
    Current,
    Root,
    OuterCurrent,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum FilterLiteral {
    String(String),
    Number(String),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SqlValue {
    Literal(u64),
    Parameter(SqlParameter),
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SqlParameter {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SqlVariantCase {
    pub value: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SelectionPlanItem {
    Projection(Projection),
    Relation(NestedRelation),
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Projection {
    pub column: ColumnId,
    pub output_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct NestedRelation {
    pub relation_name: String,
    pub output_name: String,
    pub table: TableId,
    pub foreign_key: ForeignKeyId,
    pub selections: Box<SelectionPlan>,
}

impl fmt::Display for PlanDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for PlanDiagnostic {}

impl miette::Diagnostic for PlanDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("{:?}", DsqlDiagnostic::code(self))))
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

impl DsqlDiagnostic for PlanDiagnostic {
    fn range(&self) -> TextRange {
        self.range
    }

    fn severity(&self) -> Severity {
        Severity::Error
    }

    fn code(&self) -> DiagnosticCode {
        match self.kind {
            PlanDiagnosticKind::TableNotFound { .. } => DiagnosticCode::TableNotFound,
            PlanDiagnosticKind::AmbiguousTable { .. } => DiagnosticCode::AmbiguousTable,
            PlanDiagnosticKind::UnknownFragment { .. } => DiagnosticCode::UnknownFragment,
            PlanDiagnosticKind::AmbiguousRelation { .. } => DiagnosticCode::AmbiguousRelation,
        }
    }

    fn source(&self) -> DiagnosticSource {
        DiagnosticSource::Plan
    }
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}
