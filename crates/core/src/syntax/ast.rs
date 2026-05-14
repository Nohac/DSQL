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
pub enum Expr {
    Literal(Literal),
    Name(NameRef),
    Binary {
        range: TextRange,
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum Literal {
    String { range: TextRange, value: String },
    Number { range: TextRange, value: String },
    Bool { range: TextRange, value: bool },
    Null { range: TextRange },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum BinaryOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
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
