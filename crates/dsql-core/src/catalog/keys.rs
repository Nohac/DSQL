use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
pub struct SchemaKey {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
pub struct TableKey {
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
pub struct ColumnKey {
    pub schema: String,
    pub table: String,
    pub column: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct SchemaId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct TableId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct ColumnId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct ForeignKeyId(pub usize);

/// Dense identity of one directional, query-facing catalog relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct RelationId(pub usize);
