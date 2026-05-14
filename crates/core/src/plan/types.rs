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
    pub selections: SelectionPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SelectionPlan {
    pub table: TableId,
    pub projections: Vec<Projection>,
    pub relations: Vec<NestedRelation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Projection {
    pub column: ColumnId,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct NestedRelation {
    pub field_name: String,
    pub table: TableId,
    pub foreign_key: ForeignKeyId,
    pub selections: Box<SelectionPlan>,
}
