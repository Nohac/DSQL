mod component;
mod keys;
mod lookup;
mod metadata;
mod provider;
mod types;

pub use component::{CatalogSnapshot, CatalogSourceRoot, insert_catalog};
pub use keys::{
    ColumnId, ColumnKey, ForeignKeyId, RelationId, SchemaId, SchemaKey, TableId, TableKey, TypeId,
    TypeKey,
};
pub use metadata::{
    CatalogBuildError, CatalogMissingType, CatalogTypeMismatch, ColumnMetadata, DatabaseMetadata,
    ForeignKeyConstraintMetadata, ForeignKeyReferenceMetadata, IndexKeyMetadata, IndexMetadata,
    ObjectType, SchemaMetadata, TableConstraintKind, TableConstraintMetadata, TableMetadata,
    TypeMetadata, TypeMetadataFile, metadata_from_yaml, metadata_to_yaml, table_metadata_from_yaml,
    table_metadata_to_yaml, type_metadata_file_from_yaml, type_metadata_file_to_yaml,
};
pub use types::{
    Catalog, CatalogSourceRange, CatalogSupport, CatalogSupportKind, CatalogType, Column, DataType,
    FieldCheckResult, FieldRef, ForeignKey, ForeignKeyDirection, Index, IndexKey,
    IndexKeyCapability, IndexNullsPosition, IndexOrder, IndexOrderDirection, LiteralKind, Relation,
    RelationCardinality, RelationField, RelationSupports, Schema, Table, TableRef, TableResolution,
    UniquenessSupport,
};
