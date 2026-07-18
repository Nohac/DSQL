//! Plan representation.

use bowl::Component;

use crate::catalog::{ColumnId, DataType, ForeignKeyId, TableId, TableKey};
use crate::entities::aggregate::{AggregateFunction, AggregateMode};
use crate::entities::expression::ComparisonOp;
use crate::facts::Span;
use crate::resolution::ResolvedSelectionShape;

/// One planned query root as a fact, derived per query definition by the
/// planning system, gated on [`PlanDemand`].
///
/// [`PlanDemand`]: crate::facts::PlanDemand
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct QueryPlanFact(pub QueryPlan);

/// Artifact-assembly context riding each plan entity: which definition and
/// root the plan came from, so generators can name and source-map the
/// operation without re-walking facts.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct OperationSeed {
    /// The defining query's name.
    pub query_name: String,
    /// Index of this root selection within the query definition.
    pub root_index: usize,
    /// How many root selections the definition has.
    pub root_count: usize,
    /// Span of the whole definition in its file.
    pub def_span: Span,
    /// The definition's resolution scope.
    pub scope: String,
    /// Fragment spreads the plan expanded, with the result path each sat
    /// at — provenance for generated artifacts, not consumed by SQL.
    pub spreads: Vec<SpreadUse>,
}

/// One fragment spread occurrence under a plan root: `path` is the result
/// path of the selection set the spread appeared in.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SpreadUse {
    pub path: String,
    pub fragment: String,
}

/// One planned fragment body as a fact, derived per fragment definition by
/// the planning system, gated on [`PlanDemand`]. Fragments render no SQL of
/// their own; the plan exists for result-shape derivation in generated
/// artifacts.
///
/// [`PlanDemand`]: crate::facts::PlanDemand
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct FragmentPlanFact {
    pub name: String,
    /// The catalog table the fragment is declared on.
    pub table: TableId,
    pub selections: SelectionPlan,
    pub def_span: Span,
    pub scope: String,
    /// Fragment spreads the body expanded, with the result path each sat
    /// at (the empty path is the fragment root) — provenance renderers
    /// use to compose fragment types by reuse instead of re-inlining.
    pub spreads: Vec<SpreadUse>,
}

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
    pub output_name: String,
    pub flattened: bool,
    pub collection: CollectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FragmentPlan {
    pub table: TableId,
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SelectionPlan {
    pub items: Vec<SelectionPlanItem>,
}

/// One collection source and the cardinality-changing result produced from it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CollectionPlan {
    pub table: TableId,
    /// Semantic row shape of the source selection.
    pub shape: ResolvedSelectionShape,
    pub clauses: SelectionClauses,
    pub result: CollectionResultPlan,
}

/// How one collection source becomes public result data.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CollectionResultPlan {
    Rows(SelectionPlan),
    Aggregate(AggregatePlan),
}

/// One aggregate object or grouped array.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AggregatePlan {
    pub mode: AggregateMode,
    pub group_keys: Vec<AggregateGroupProjection>,
    pub fields: Vec<AggregateProjection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AggregateGroupProjection {
    pub column: ColumnId,
    pub output_name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

/// One computed scalar inside an [`AggregatePlan`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AggregateProjection {
    pub function: AggregateFunction,
    pub operand: Option<ColumnId>,
    pub output_name: String,
    pub data_type: DataType,
    pub nullable: bool,
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
    Not(Box<FilterExpr>),
    NullTest {
        operand: Box<FilterExpr>,
        negated: bool,
    },
    Membership {
        operand: Box<FilterExpr>,
        collection: FilterCollection,
        negated: bool,
    },
    VariantBinary {
        left: Box<FilterExpr>,
        path: String,
        variants: Vec<SqlVariantCase>,
        right: Box<FilterExpr>,
    },
    Exists {
        foreign_key: Option<ForeignKeyId>,
        table: TableId,
        kind: ExistsKind,
        filter: Option<Box<FilterExpr>>,
    },
    /// One correlated scalar aggregate over a direct relation.
    RelationAggregate {
        foreign_key: ForeignKeyId,
        table: TableId,
        function: AggregateFunction,
        operand: Option<ColumnId>,
    },
}

/// Whether an existence node was written explicitly or introduced while
/// lowering a relationship-qualified predicate path.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ExistsKind {
    Explicit,
    RelationshipPredicate,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterCollection {
    List(Vec<FilterExpr>),
    Parameter(SqlParameter),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterColumnScope {
    Current,
    Root,
    PredicateSource,
    Parent,
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
    pub flattened: bool,
    pub foreign_key: ForeignKeyId,
    pub collection: Box<CollectionPlan>,
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}
