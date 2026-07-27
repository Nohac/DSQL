use facet::Facet;

/// Schema-qualified identity of one provider type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
pub struct TypeKey {
    /// Provider schema containing the type.
    pub schema: String,
    /// Provider-internal type name.
    pub name: String,
}

impl TypeKey {
    /// Creates a schema-qualified provider type identity.
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }
}

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

/// Dense identity of one type in the effective catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct TypeId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct ForeignKeyId(pub usize);

/// Dense identity of one directional, query-facing catalog relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct RelationId(pub usize);
