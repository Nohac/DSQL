use crate::{
    catalog::{ColumnId, ForeignKeyId, TableId},
    syntax::Diagnostic,
};
use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct PlannedFile {
    pub queries: Vec<QueryPlan>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct QueryPlan {
    pub root: TableId,
    pub output_name: String,
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SelectionPlan {
    pub table: TableId,
    pub items: Vec<SelectionPlanItem>,
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
