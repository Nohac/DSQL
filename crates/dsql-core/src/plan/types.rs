//! Plan representation.

use bowl::{Component, Entity};

use crate::catalog::{ColumnId, DataType, RelationId, TableId, TableKey, TypeKey, WireEncoding};
use crate::entities::aggregate::{AggregateFunction, AggregateMode};
use crate::entities::expression::ComparisonOp;
use crate::entities::expression::DynamicInputSurface;
use crate::facts::Span;
use crate::resolution::ResolvedSelectionShape;

/// One planned query definition as a fact, derived by the planning system
/// and gated on [`PlanDemand`].
///
/// [`PlanDemand`]: crate::facts::PlanDemand
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct QueryPlanFact(pub QueryPlan);

/// Artifact-assembly context riding each plan entity, so generators can
/// source-map the operation without re-walking facts.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct OperationSeed {
    /// The defining query's name.
    pub query_name: String,
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
    /// Project-wide field targets that can be masked on this table. Fragment
    /// artifacts are shared across resolution scopes, so their public type
    /// contract is deliberately conservative even when a filter is not
    /// visible from the fragment's declaring scope.
    pub policy_nullable_fields: Vec<PolicyFieldTarget>,
    /// Project-wide conservative access classes for fields this reusable
    /// fragment may expose in any importing scope.
    pub policy_field_access: Vec<PolicyFieldAccess>,
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
    /// Root selections in source order.
    pub roots: Vec<QueryRootPlan>,
    /// Server-only values required by policies that can affect any root.
    pub policy_context: Vec<PolicyContextRequirement>,
    /// Normalized public capability contracts keyed by full input path.
    pub dynamic_inputs: Vec<DynamicInputContract>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueryRootPlan {
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
    /// Raw-view row constraint composed from the effective policy state.
    pub policy_filter: Option<FilterExpr>,
    /// Effective, scope-specific readable-view guards. These drive SQL
    /// enforcement and its trusted-context parameters.
    pub field_filters: Vec<PolicyFieldFilter>,
    /// Project-wide field targets that can be masked. This separate,
    /// conservative set drives generated nullability contracts so one shared
    /// fragment artifact remains sound in every importing scope.
    pub policy_nullable_fields: Vec<PolicyFieldTarget>,
    /// Conservative project-wide access classes used by shared fragment
    /// result contracts.
    pub policy_field_access: Vec<PolicyFieldAccess>,
    /// Identity-preserving audit data for every filter observed while
    /// planning this source and its predicate-only traversals.
    pub policy_applications: Vec<PolicyApplicationPlan>,
    pub result: CollectionResultPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PolicyFieldFilter {
    pub target: PolicyFieldTarget,
    pub filter: FilterExpr,
    /// Trusted context required when this guard is actually rendered. SQL
    /// generation carries only reached requirements into artifact metadata.
    pub context: Vec<PolicyContextRequirement>,
}

/// Consumer-facing access classification ordered by increasing dependence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PolicyAccess {
    Unconditional,
    ContextOnly,
    RowDependent,
}

impl PolicyAccess {
    /// Classifies one final readable-view guard. A literal `true` cannot mask;
    /// catalog observations make the result row-dependent; other guards are
    /// decidable from execution inputs and trusted context.
    pub fn for_guard(filter: &FilterExpr) -> Self {
        if matches!(filter, FilterExpr::Literal(FilterLiteral::Bool(true))) {
            Self::Unconditional
        } else if filter_observes_rows(filter) {
            Self::RowDependent
        } else {
            Self::ContextOnly
        }
    }

    /// Composes independent guards conservatively: row dependence dominates
    /// context-only access, which dominates unconditional access.
    pub fn combine(self, other: Self) -> Self {
        self.max(other)
    }
}

/// One project-wide field target and its most dependent possible guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PolicyFieldAccess {
    pub target: PolicyFieldTarget,
    pub access: PolicyAccess,
}

/// How query source selected a filter's desired state at one source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PolicyAssignmentState {
    Default,
    Enabled,
    Disabled,
    Conditional,
}

/// Whether trusted context can force one filter active.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PolicyEnforcement {
    None,
    Always,
    Conditional,
}

/// Stable scope-qualified policy identity carried from resolution into audit
/// metadata.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PolicyIdentity {
    pub scope: String,
    pub name: String,
}

/// One logical field affected by an active policy application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PolicyApplicationField {
    pub target: PolicyFieldTarget,
    pub access: PolicyAccess,
}

/// One filter's effective state at one observed catalog source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PolicyApplicationPlan {
    /// Bowl identity retained only to join declaration provenance during
    /// metadata assembly.
    pub filter: Entity,
    pub identity: PolicyIdentity,
    pub conditions: Vec<PolicyIdentity>,
    pub path: String,
    pub target: TableId,
    pub default_active: bool,
    pub enforcement: PolicyEnforcement,
    pub assignment: PolicyAssignmentState,
    pub rows_filtered: bool,
    pub fields: Vec<PolicyApplicationField>,
    pub context: Vec<PolicyContextRequirement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolicyFieldTarget {
    Column(ColumnId),
    Relation(RelationId),
}

fn filter_observes_rows(filter: &FilterExpr) -> bool {
    match filter {
        FilterExpr::Column { .. }
        | FilterExpr::Exists { .. }
        | FilterExpr::RelationAggregate { .. }
        | FilterExpr::DynamicPredicate { .. } => true,
        FilterExpr::Binary { left, right, .. } | FilterExpr::VariantBinary { left, right, .. } => {
            filter_observes_rows(left) || filter_observes_rows(right)
        }
        FilterExpr::Optional { operand, .. }
        | FilterExpr::Not(operand)
        | FilterExpr::NullTest { operand, .. } => filter_observes_rows(operand),
        FilterExpr::Membership {
            operand,
            collection,
            ..
        } => {
            filter_observes_rows(operand)
                || match collection {
                    FilterCollection::List(items) => items.iter().any(filter_observes_rows),
                    FilterCollection::Parameter(_) => false,
                }
        }
        FilterExpr::Absent | FilterExpr::Literal(_) | FilterExpr::Parameter(_) => false,
    }
}

/// One trusted context value inferred while compiling a policy expression.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PolicyContextRequirement {
    pub path: String,
    pub data_type: DataType,
    pub wire: WireEncoding,
    pub provider_type: Option<TypeKey>,
    pub collection: bool,
}

impl PolicyContextRequirement {
    /// Whether two uses of one trusted-context path require incompatible values.
    pub(crate) fn conflicts_with(&self, other: &Self) -> bool {
        if self.collection != other.collection {
            return true;
        }
        if self.wire == WireEncoding::TextCast || other.wire == WireEncoding::TextCast {
            return self.wire != other.wire || self.provider_type != other.provider_type;
        }
        self.data_type != other.data_type || self.wire != other.wire
    }

    pub(crate) fn conflict_message(&self, other: &Self) -> String {
        format!(
            "trusted context `{}` is required as both {} and {}",
            self.path,
            self.shape(),
            other.shape()
        )
    }

    fn shape(&self) -> String {
        let data_type = self
            .provider_type
            .as_ref()
            .filter(|_| self.wire == WireEncoding::TextCast)
            .map_or_else(
                || self.data_type.as_str().to_string(),
                |key| format!("{}.{}", key.schema, key.name),
            );
        if self.collection {
            format!("a collection of `{data_type}`")
        } else {
            format!("`{data_type}`")
        }
    }
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

/// One normalized bounded dynamic public-input contract.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DynamicInputContract {
    pub path: String,
    pub kind: DynamicInputKind,
    pub surface: DynamicInputSurface,
    pub fields: Vec<DynamicInputFieldPlan>,
}

/// The runtime structure bounded by one dynamic input contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynamicInputKind {
    Predicate,
    Order,
}

/// One scalar exposed by a bounded dynamic capability preset.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DynamicInputFieldPlan {
    pub key: String,
    pub column: ColumnId,
    pub data_type: DataType,
    pub nullable: bool,
    pub access: PolicyAccess,
    pub operators: Vec<DynamicPredicateOperator>,
    pub directions: Vec<DynamicOrderDirection>,
}

/// One compiler-owned predicate operation exposed for a preset scalar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynamicPredicateOperator {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    In,
    NotIn,
    IsNull,
}

impl DynamicPredicateOperator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Neq => "neq",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::Like => "like",
            Self::In => "in",
            Self::NotIn => "not_in",
            Self::IsNull => "is_null",
        }
    }
}

/// One public direction accepted by a bounded dynamic order entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynamicOrderDirection {
    Asc,
    Desc,
    AscNullsFirst,
    AscNullsLast,
    DescNullsFirst,
    DescNullsLast,
}

impl DynamicOrderDirection {
    /// Canonical source and generated-metadata spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
            Self::AscNullsFirst => "asc_nulls_first",
            Self::AscNullsLast => "asc_nulls_last",
            Self::DescNullsFirst => "desc_nulls_first",
            Self::DescNullsLast => "desc_nulls_last",
        }
    }

    /// Every accepted direction in deterministic public order.
    pub const ALL: [Self; 6] = [
        Self::Asc,
        Self::Desc,
        Self::AscNullsFirst,
        Self::AscNullsLast,
        Self::DescNullsFirst,
        Self::DescNullsLast,
    ];
}

/// One static or runtime-expanded ordering term in source precedence order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OrderByPlan {
    Column {
        column: ColumnId,
        direction: SortDirectionPlan,
    },
    Dynamic {
        path: String,
        surface: DynamicInputSurface,
        fields: Vec<DynamicInputFieldPlan>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SortDirectionPlan {
    Asc,
    Desc,
    Variant {
        path: String,
        variants: Vec<SqlVariantCase>,
        nullable: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterExpr {
    /// A statically omitted predicate atom (for an omitted `null` default).
    Absent,
    /// A query-authored atom present only while its public input is non-null.
    Optional {
        parameter: SqlParameter,
        operand: Box<FilterExpr>,
    },
    Column {
        scope: FilterColumnScope,
        column: ColumnId,
    },
    Literal(FilterLiteral),
    Parameter(SqlParameter),
    /// One bounded predicate object rendered by the execution runtime.
    DynamicPredicate {
        path: String,
        surface: DynamicInputSurface,
        fields: Vec<DynamicInputFieldPlan>,
    },
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
        relation: Option<RelationId>,
        table: TableId,
        kind: ExistsKind,
        /// Row whose relation connects to this source.
        source_scope: FilterColumnScope,
        /// Effective raw-view row policies for the observed source.
        policy_filter: Option<Box<FilterExpr>>,
        /// Effective readable-view field policies for the observed source.
        field_filters: Vec<PolicyFieldFilter>,
        filter: Option<Box<FilterExpr>>,
    },
    /// One correlated scalar aggregate over a direct relation.
    RelationAggregate {
        relation: RelationId,
        table: TableId,
        function: AggregateFunction,
        operand: Option<ColumnId>,
        /// Effective raw-view row policies for the aggregate source.
        policy_filter: Option<Box<FilterExpr>>,
        /// Effective readable-view field policies for the aggregate source.
        field_filters: Vec<PolicyFieldFilter>,
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
    pub text_cast: Option<TypeKey>,
    pub collection: bool,
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
    pub relation: RelationId,
    pub collection: Box<CollectionPlan>,
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}
