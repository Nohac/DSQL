use crate::{
    catalog::{ColumnId, ForeignKeyId, TableId},
    syntax::{BinaryOp, Diagnostic},
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
    pub clauses: SelectionClauses,
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
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct OrderByPlan {
    pub column: ColumnId,
    pub direction: SortDirectionPlan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum SortDirectionPlan {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum FilterExpr {
    Column {
        scope: FilterColumnScope,
        column: ColumnId,
    },
    Literal(FilterLiteral),
    Binary {
        left: Box<FilterExpr>,
        op: BinaryOp,
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
