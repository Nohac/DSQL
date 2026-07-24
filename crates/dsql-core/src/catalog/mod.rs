mod component;
mod keys;
mod lookup;
mod metadata;
mod provider;
mod types;

pub use component::{CatalogSnapshot, CatalogSourceRoot, insert_catalog};
pub use keys::{
    ColumnId, ColumnKey, ForeignKeyId, RelationId, SchemaId, SchemaKey, TableId, TableKey,
};
pub use metadata::{
    CatalogBuildError, ColumnMetadata, DatabaseMetadata, ForeignKeyConstraintMetadata,
    ForeignKeyReferenceMetadata, IndexMetadata, ObjectType, SchemaMetadata, TableConstraintKind,
    TableConstraintMetadata, TableMetadata, TypeMetadata, TypeMetadataFile, metadata_from_yaml,
    metadata_to_yaml, table_metadata_from_yaml, table_metadata_to_yaml,
    type_metadata_file_from_yaml, type_metadata_file_to_yaml,
};
pub use types::{
    Catalog, CatalogSourceRange, CatalogSupport, CatalogSupportKind, Column, DataType,
    FieldCheckResult, FieldRef, ForeignKey, ForeignKeyDirection, Index, LiteralKind, Relation,
    RelationCardinality, RelationField, RelationSupports, Schema, Table, TableRef, TableResolution,
    UniquenessSupport,
};
