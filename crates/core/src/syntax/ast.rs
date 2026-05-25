use super::TextRange;
use facet::Facet;

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct Document {
    pub definitions: Vec<Definition>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum Definition {
    Query(QueryDef),
    Fragment(FragmentDef),
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct QueryDef {
    pub range: TextRange,
    pub name: Option<NameRef>,
    pub selections: Vec<Selection>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct FragmentDef {
    pub range: TextRange,
    pub name: Option<NameRef>,
    pub on: Option<NameRef>,
    pub selections: Vec<Selection>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct Selection {
    pub range: TextRange,
    pub kind: SelectionKind,
    pub alias: Option<NameRef>,
    pub name: NameRef,
    pub arguments: Vec<Argument>,
    pub has_clause_list: bool,
    pub clauses: Vec<Clause>,
    pub directives: Vec<NameRef>,
    pub selections: Vec<Selection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum SelectionKind {
    Field,
    FragmentSpread,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct Argument {
    pub range: TextRange,
    pub name: NameRef,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum Clause {
    Where(WhereClause),
    OrderBy(OrderByClause),
    Limit(LimitClause),
    Offset(OffsetClause),
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct WhereClause {
    pub range: TextRange,
    pub predicate: Expr,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct OrderByClause {
    pub range: TextRange,
    pub items: Vec<OrderByItem>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct OrderByItem {
    pub range: TextRange,
    pub field: NameRef,
    pub direction: SortDirectionExpr,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SortDirectionExpr {
    Static(SortDirection),
    Variable(ValueVariable),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet, strum::AsRefStr)]
#[repr(u8)]
#[strum(serialize_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub const ALL: &'static [Self] = &[Self::Asc, Self::Desc];

    pub fn label(&self) -> &str {
        self.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct LimitClause {
    pub range: TextRange,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct OffsetClause {
    pub range: TextRange,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum Expr {
    Literal(Literal),
    Name(NameRef),
    Path(ScopedPath),
    Variable(ValueVariable),
    Binary {
        range: TextRange,
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ValueVariable {
    pub range: TextRange,
    pub scope: VariableScope,
    pub name: Option<NameRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum VariableScope {
    Structured,
    TopLevel,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum BinaryOperator {
    Static(BinaryOp),
    Variable(OperatorVariable),
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct OperatorVariable {
    pub range: TextRange,
    pub scope: VariableScope,
    pub name: Option<NameRef>,
    pub allowed: Vec<BinaryOp>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct ScopedPath {
    pub range: TextRange,
    pub scope: PathScope,
    pub segments: Vec<ScopedPathSegment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum PathScope {
    Current,
    Parent,
    Root,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ScopedPathSegment {
    pub range: TextRange,
    pub name: NameRef,
    pub selector: Option<NameRef>,
}

impl ScopedPathSegment {
    pub fn field_ref(&self) -> String {
        self.selector.as_ref().map_or_else(
            || self.name.text.clone(),
            |selector| format!("{}::{}", self.name.text, selector.text),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum Literal {
    String { range: TextRange, value: String },
    Number { range: TextRange, value: String },
    Bool { range: TextRange, value: bool },
    Null { range: TextRange },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet, strum::AsRefStr)]
#[repr(u8)]
pub enum BinaryOp {
    #[strum(serialize = "==")]
    Eq,
    #[strum(serialize = "!=")]
    Ne,
    #[strum(serialize = ">")]
    Gt,
    #[strum(serialize = ">=")]
    Ge,
    #[strum(serialize = "<")]
    Lt,
    #[strum(serialize = "<=")]
    Le,
    #[strum(serialize = "like")]
    Like,
    #[strum(serialize = "and")]
    And,
    #[strum(serialize = "or")]
    Or,
}

impl BinaryOp {
    pub fn dsql_label(&self) -> Option<&str> {
        match self {
            Self::Eq | Self::Ne | Self::Gt | Self::Ge | Self::Lt | Self::Le | Self::Like => {
                Some(self.as_ref())
            }
            Self::And | Self::Or => None,
        }
    }

    pub fn label(&self) -> Option<&str> {
        self.dsql_label()
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::Eq => "equals",
            Self::Ne => "not equals",
            Self::Gt => "greater than",
            Self::Ge => "greater than or equal",
            Self::Lt => "less than",
            Self::Le => "less than or equal",
            Self::Like => "matches pattern",
            Self::And => "combine predicates",
            Self::Or => "match either predicate",
        }
    }

    pub const fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Gt | Self::Ge | Self::Lt | Self::Le | Self::Like
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct NameRef {
    pub range: TextRange,
    pub text: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SourceFile {
    document: Document,
}

impl SourceFile {
    pub fn new(document: Document) -> Self {
        Self { document }
    }

    pub fn definitions(&self) -> impl Iterator<Item = &Definition> {
        self.document.definitions.iter()
    }

    pub fn queries(&self) -> impl Iterator<Item = &QueryDef> {
        self.document
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Query(query) => Some(query),
                Definition::Fragment(_) => None,
            })
    }

    pub fn fragments(&self) -> impl Iterator<Item = &FragmentDef> {
        self.document
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Query(_) => None,
                Definition::Fragment(fragment) => Some(fragment),
            })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }
}

impl QueryDef {
    pub fn name(&self) -> Option<&NameRef> {
        self.name.as_ref()
    }

    pub fn selections(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }
}

impl FragmentDef {
    pub fn name(&self) -> Option<&NameRef> {
        self.name.as_ref()
    }

    pub fn selections(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }
}
