//! Plan representation.

use bowl::Component;

use crate::catalog::{ColumnId, ForeignKeyId, TableId, TableKey};
use crate::entities::expression::ComparisonOp;
use crate::facts::Span;

/// One planned query root as a fact, derived per query definition by the
/// planning system, gated on [`PlanDemand`].
///
/// [`PlanDemand`]: crate::facts::PlanDemand
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct QueryPlanFact(pub QueryPlan);

/// Binary operator inside a [`FilterExpr`]: comparisons plus the boolean
/// connectives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Like,
    And,
    Or,
}

impl From<ComparisonOp> for FilterOp {
    fn from(op: ComparisonOp) -> Self {
        match op {
            ComparisonOp::Eq => Self::Eq,
            ComparisonOp::Ne => Self::Ne,
            ComparisonOp::Gt => Self::Gt,
            ComparisonOp::Ge => Self::Ge,
            ComparisonOp::Lt => Self::Lt,
            ComparisonOp::Le => Self::Le,
            ComparisonOp::Like => Self::Like,
        }
    }
}

impl FilterOp {
    /// The dsql spelling, for operator-variant case values. `None` for the
    /// connectives, which operator variables cannot choose.
    pub fn dsql_label(self) -> Option<&'static str> {
        match self {
            Self::Eq => Some("=="),
            Self::Ne => Some("!="),
            Self::Gt => Some(">"),
            Self::Ge => Some(">="),
            Self::Lt => Some("<"),
            Self::Le => Some("<="),
            Self::Like => Some("like"),
            Self::And | Self::Or => None,
        }
    }

    /// The PostgreSQL spelling, for operator-variant case texts.
    pub fn postgres_text(self) -> Option<&'static str> {
        match self {
            Self::Eq => Some("="),
            Self::Ne => Some("!="),
            Self::Gt => Some(">"),
            Self::Ge => Some(">="),
            Self::Lt => Some("<"),
            Self::Le => Some("<="),
            Self::Like => Some("like"),
            Self::And | Self::Or => None,
        }
    }
}


#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlannedFile {
    pub queries: Vec<QueryPlan>,
    pub diagnostics: Vec<PlanDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlanDiagnostic {
    pub span: Span,
    pub kind: PlanDiagnosticKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueryPlan {
    pub root: TableId,
    pub output_name: String,
    pub clauses: SelectionClauses,
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FragmentPlan {
    pub table: TableId,
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SelectionPlan {
    pub table: TableId,
    pub clauses: SelectionClauses,
    pub items: Vec<SelectionPlanItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SelectionClauses {
    pub filter: Option<FilterExpr>,
    pub order_by: Vec<OrderByPlan>,
    pub limit: Option<SqlValue>,
    pub offset: Option<SqlValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OrderByPlan {
    pub column: ColumnId,
    pub direction: SortDirectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SortDirectionPlan {
    Asc,
    Desc,
    Variant {
        path: String,
        variants: Vec<SqlVariantCase>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterExpr {
    Column {
        scope: FilterColumnScope,
        column: ColumnId,
    },
    Literal(FilterLiteral),
    Parameter(SqlParameter),
    Binary {
        left: Box<FilterExpr>,
        op: FilterOp,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterColumnScope {
    Current,
    Root,
    OuterCurrent,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterLiteral {
    String(String),
    Number(String),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SqlValue {
    Literal(u64),
    Parameter(SqlParameter),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SqlParameter {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SqlVariantCase {
    pub value: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelectionPlanItem {
    Projection(Projection),
    Relation(NestedRelation),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Projection {
    pub column: ColumnId,
    pub output_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NestedRelation {
    pub relation_name: String,
    pub output_name: String,
    pub table: TableId,
    pub foreign_key: ForeignKeyId,
    pub selections: Box<SelectionPlan>,
}


fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}
