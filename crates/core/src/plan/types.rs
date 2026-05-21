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
