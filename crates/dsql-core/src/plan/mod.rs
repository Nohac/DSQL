//! Query planning: the plan representation and the facts-to-plan builder.

mod build;
mod types;

pub use build::register_planning;
pub use types::{
    FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, FragmentPlan, NestedRelation,
    FragmentPlanFact, OperationSeed, OrderByPlan, PlanDiagnostic, PlanDiagnosticKind, SpreadUse,
    PlannedFile, Projection, QueryPlan, QueryPlanFact, SelectionClauses, SelectionPlan,
    SelectionPlanItem, SortDirectionPlan,
    SqlParameter, SqlValue, SqlVariantCase,
};
