//! Query planning: the plan representation and the facts-to-plan builder.

mod build;
mod types;

pub use build::register_planning;
pub use types::{
    AggregatePlan, AggregateProjection, CollectionPlan, CollectionResultPlan, ExistsKind,
    FilterCollection, FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, FragmentPlan,
    FragmentPlanFact, NestedRelation, OperationSeed, OrderByPlan, PlanDiagnostic,
    PlanDiagnosticKind, PlannedFile, PolicyAccess, PolicyApplicationField, PolicyApplicationPlan,
    PolicyAssignmentState, PolicyContextRequirement, PolicyEnforcement, PolicyFieldAccess,
    PolicyFieldFilter, PolicyFieldTarget, PolicyIdentity, Projection, QueryPlan, QueryPlanFact,
    SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan, SpreadUse, SqlParameter,
    SqlValue, SqlVariantCase,
};
