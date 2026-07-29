use super::{
    Catalog, CatalogEnum, CatalogEnumVariant, CatalogType, CatalogTypeShape, Column, ColumnId,
    DataType, ForeignKey, ForeignKeyDirection, ForeignKeyId, Index, IndexKey, IndexKeyCapability,
    IndexOrder, Relation, RelationCardinality, RelationId, RelationSupports, Schema, SchemaId,
    Table, TableId, TypeCapabilities, TypeId, TypeKey,
};
use facet::Facet;
use std::collections::{BTreeSet, HashMap, HashSet};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct DatabaseMetadata {
    pub schemas: Vec<SchemaMetadata>,
    pub types: Vec<TypeMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SchemaMetadata {
    pub name: String,
    pub tables: Vec<TableMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct TableMetadata {
    pub schema: String,
    pub name: String,
    pub object_type: ObjectType,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub description: Option<String>,
    pub columns: Vec<ColumnMetadata>,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub constraints: Vec<TableConstraintMetadata>,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub foreign_keys: Vec<ForeignKeyConstraintMetadata>,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub indexes: Vec<IndexMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ColumnMetadata {
    pub name: String,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub description: Option<String>,
    /// Schema-qualified provider identity of this column's type.
    pub provider_type: TypeKey,
    /// Exact provider display spelling, including modifiers and qualification.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub formatted_type: Option<String>,
    /// Raw PostgreSQL `atttypmod`, when supplied by introspection.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub type_modifier: Option<i32>,
    /// Internal PostgreSQL `typname`, used for compiler logical classification.
    pub database_type: String,
    pub data_type: DataType,
    pub not_null: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct TableConstraintMetadata {
    #[facet(skip_serializing_if = Option::is_none)]
    pub name: Option<String>,
    pub kind: TableConstraintKind,
    pub columns: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet, strum::AsRefStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum TableConstraintKind {
    PrimaryKey,
    Unique,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ForeignKeyConstraintMetadata {
    #[facet(skip_serializing_if = Option::is_none)]
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub references: ForeignKeyReferenceMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ForeignKeyReferenceMetadata {
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct IndexMetadata {
    #[facet(skip_serializing_if = Option::is_none)]
    pub name: Option<String>,
    pub access_method: String,
    pub keys: Vec<IndexKeyMetadata>,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub included_columns: Vec<String>,
    pub unique: bool,
}

/// One ordered provider index key before catalog identity resolution.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct IndexKeyMetadata {
    pub column: String,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub operator_class: Option<String>,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub capabilities: Vec<IndexKeyCapability>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub order: Option<IndexOrder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct TypeMetadata {
    pub internal_type: String,
    pub readable_type: String,
    pub schema: String,
    /// Provider-neutral structural relationship for this type.
    pub structure: TypeStructureMetadata,
    /// Raw provider facts from the same PostgreSQL catalog snapshot.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub provider: Option<ProviderTypeFacts>,
    pub operations: BTreeSet<String>,
}

/// Structural identity of one provider type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct TypeStructureMetadata {
    pub kind: TypeStructureKind,
    /// Base type for a domain or element type for an array.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub related_type: Option<TypeKey>,
    /// Native enum data, present only when [`TypeStructureKind::Enum`].
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub enumeration: Option<EnumTypeMetadata>,
}

/// Generated catalog facts for one native PostgreSQL enum.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct EnumTypeMetadata {
    /// PostgreSQL type comment, when one exists.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub description: Option<String>,
    /// Variants in PostgreSQL semantic order.
    pub variants: Vec<EnumVariantMetadata>,
}

/// One native PostgreSQL enum label and its database representation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct EnumVariantMetadata {
    /// Stable value exposed by DSQL and generated APIs.
    pub variant: String,
    /// Value accepted and returned by PostgreSQL.
    pub database_value: String,
    /// Optional human-facing label reserved for normalized enum sources.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub label: Option<String>,
    /// Optional per-variant documentation.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub description: Option<String>,
}

impl TypeStructureMetadata {
    /// Constructs a scalar type with no structural dependency.
    pub fn scalar() -> Self {
        Self {
            kind: TypeStructureKind::Scalar,
            related_type: None,
            enumeration: None,
        }
    }

    /// Constructs a domain over `base`.
    pub fn domain(base: TypeKey) -> Self {
        Self {
            kind: TypeStructureKind::Domain,
            related_type: Some(base),
            enumeration: None,
        }
    }

    /// Constructs an array of `element`.
    pub fn array(element: TypeKey) -> Self {
        Self {
            kind: TypeStructureKind::Array,
            related_type: Some(element),
            enumeration: None,
        }
    }

    /// Constructs a native enum with its ordered provider labels.
    pub fn enumeration(enumeration: EnumTypeMetadata) -> Self {
        Self {
            kind: TypeStructureKind::Enum,
            related_type: None,
            enumeration: Some(enumeration),
        }
    }
}

/// Structural category retained independently of logical scalar mappings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet, strum::AsRefStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum TypeStructureKind {
    Scalar,
    Domain,
    Array,
    Enum,
}

/// Native PostgreSQL classification and ordering facts for one type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct ProviderTypeFacts {
    /// Raw one-character PostgreSQL `pg_type.typtype` code.
    pub kind: String,
    /// Raw one-character PostgreSQL `pg_type.typcategory` code.
    pub category: String,
    /// Raw one-character kind of the terminal non-domain provider type.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub effective_kind: Option<String>,
    /// Raw one-character category of the terminal non-domain provider type.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub effective_category: Option<String>,
    /// Whether an applicable default btree operator class exists.
    pub orderable: bool,
}

impl ProviderTypeFacts {
    pub(crate) fn supports_text_cast(&self) -> bool {
        let kind = self.effective_kind.as_deref().unwrap_or(&self.kind);
        let category = self.effective_category.as_deref().unwrap_or(&self.category);
        matches!(kind, "b" | "e") && category != "A"
    }
}

impl TypeMetadata {
    /// Returns the schema-qualified provider identity described by this row.
    pub fn key(&self) -> TypeKey {
        TypeKey {
            schema: self.schema.clone(),
            name: self.internal_type.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum ObjectType {
    Table,
    View,
    MaterializedView,
    Index,
    Sequence,
    Special,
    ToastTable,
    ForeignTable,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct TypeMetadataFile {
    pub types: Vec<TypeMetadata>,
}

#[derive(Debug, Error)]
pub enum CatalogBuildError {
    #[error("duplicate schema `{schema}`")]
    DuplicateSchema { schema: String },
    #[error("duplicate table `{schema}.{table}`")]
    DuplicateTable { schema: String, table: String },
    #[error("duplicate column `{schema}.{table}.{column}`")]
    DuplicateColumn {
        schema: String,
        table: String,
        column: String,
    },
    #[error("duplicate provider type `{schema}.{name}`")]
    DuplicateType { schema: String, name: String },
    #[error("provider type `{schema}.{name}` has invalid structure: {message}")]
    InvalidTypeStructure {
        schema: String,
        name: String,
        message: String,
    },
    #[error(
        "provider type `{schema}.{name}` references missing {relation} type `{target_schema}.{target_name}`"
    )]
    MissingRelatedType {
        schema: String,
        name: String,
        relation: &'static str,
        target_schema: String,
        target_name: String,
    },
    #[error("provider type structure contains a cycle through `{schema}.{name}`")]
    CyclicTypeStructure { schema: String, name: String },
    #[error(
        "provider type `{schema}.{name}` is a native enum stored without native enum structure; rerun `dsql introspect`"
    )]
    StaleNativeEnum { schema: String, name: String },
    #[error(transparent)]
    MissingType(Box<CatalogMissingType>),
    #[error(transparent)]
    TypeMismatch(Box<CatalogTypeMismatch>),
    #[error(
        "provider type `{schema}.{name}` has conflicting fixture logical types `{first}` and `{second}`"
    )]
    ConflictingFixtureType {
        schema: String,
        name: String,
        first: String,
        second: String,
    },
    #[error("column `{schema}.{table}.{column}` was not found")]
    MissingColumn {
        schema: String,
        table: String,
        column: String,
    },
    #[error("column set for `{schema}.{table}.{name}` is empty")]
    EmptyColumnSet {
        schema: String,
        table: String,
        name: String,
    },
    #[error(
        "foreign key `{schema}.{table}.{name}` maps {columns} local columns to {referenced_columns} referenced columns"
    )]
    ForeignKeyColumnCountMismatch {
        schema: String,
        table: String,
        name: String,
        columns: usize,
        referenced_columns: usize,
    },
    #[error("foreign key target `{schema}.{table}.{column}` was not found")]
    MissingForeignKeyTarget {
        schema: String,
        table: String,
        column: String,
    },
}

/// Display-only catalog-construction payload owned by `dsql-core`.
///
/// Callers match [`CatalogBuildError`] rather than destructuring these
/// internals. The payload is boxed to keep the public error enum small.
#[derive(Debug, Error)]
#[error(
    "column `{schema}.{table}.{column}` references missing provider type `{type_schema}.{type_name}`; rerun `dsql introspect`"
)]
pub struct CatalogMissingType {
    schema: String,
    table: String,
    column: String,
    type_schema: String,
    type_name: String,
}

/// Display-only catalog-construction payload owned by `dsql-core`.
///
/// Callers match [`CatalogBuildError`] rather than destructuring these
/// internals. The payload is boxed to keep the public error enum small.
#[derive(Debug, Error)]
#[error(
    "column `{schema}.{table}.{column}` declares logical type `{declared}` but provider type `{type_schema}.{type_name}` maps to `{resolved}`; rerun `dsql introspect`"
)]
pub struct CatalogTypeMismatch {
    schema: String,
    table: String,
    column: String,
    type_schema: String,
    type_name: String,
    declared: String,
    resolved: String,
}

pub fn metadata_from_yaml(input: &str) -> Result<DatabaseMetadata, facet_yaml::DeserializeError> {
    facet_yaml::from_str(input)
}

pub fn metadata_to_yaml(metadata: &DatabaseMetadata) -> Result<String, String> {
    serialize_yaml(metadata)
}

pub fn table_metadata_from_yaml(
    input: &str,
) -> Result<TableMetadata, facet_yaml::DeserializeError> {
    facet_yaml::from_str(input)
}

pub fn table_metadata_to_yaml(table: &TableMetadata) -> Result<String, String> {
    serialize_yaml(table)
}

pub fn type_metadata_file_from_yaml(
    input: &str,
) -> Result<TypeMetadataFile, facet_yaml::DeserializeError> {
    facet_yaml::from_str(input)
}

pub fn type_metadata_file_to_yaml(types: &TypeMetadataFile) -> Result<String, String> {
    serialize_yaml(types)
}

fn serialize_yaml<T: Facet<'static>>(value: &T) -> Result<String, String> {
    // facet_yaml currently leaves a space after mapping keys whose values start
    // on the next line; normalize that serializer quirk before publication.
    facet_yaml::to_string(value)
        .map(|yaml| yaml.replace(": \n", ":\n"))
        .map_err(|error| error.to_string())
}

impl DatabaseMetadata {
    /// Builds the provider catalog without consuming the decoded metadata.
    pub fn to_catalog(&self) -> Result<Catalog, CatalogBuildError> {
        Catalog::try_from_metadata(self)
    }

    pub fn canonicalize(&mut self) {
        self.schemas
            .sort_by(|left, right| left.name.cmp(&right.name));
        for schema in &mut self.schemas {
            schema
                .tables
                .sort_by(|left, right| left.name.cmp(&right.name));
        }
        self.types.sort_by_key(TypeMetadata::key);
    }
}

impl Catalog {
    pub fn try_from_metadata(metadata: &DatabaseMetadata) -> Result<Self, CatalogBuildError> {
        let mut schemas = Vec::new();
        let mut tables = Vec::new();
        let (types, type_ids) = build_catalog_types(metadata)?;
        let mut columns = Vec::new();
        let mut foreign_keys = Vec::new();
        let mut schema_ids = HashMap::<String, SchemaId>::new();
        let mut table_ids = HashMap::<(String, String), TableId>::new();
        let mut column_ids = HashMap::<(String, String, String), ColumnId>::new();
        let mut table_columns = Vec::<Vec<ColumnId>>::new();
        let mut table_primary_keys = Vec::<Vec<ColumnId>>::new();
        let mut table_unique_constraints = Vec::<Vec<Vec<ColumnId>>>::new();
        let mut table_indexes = Vec::<Vec<Index>>::new();
        for schema_metadata in &metadata.schemas {
            if schema_ids.contains_key(&schema_metadata.name) {
                return Err(CatalogBuildError::DuplicateSchema {
                    schema: schema_metadata.name.clone(),
                });
            }
            let schema_id = SchemaId(schemas.len());
            schema_ids.insert(schema_metadata.name.clone(), schema_id);
            schemas.push(Schema::new(schema_id, &schema_metadata.name, Vec::new()));
        }

        for schema_metadata in &metadata.schemas {
            let schema_id = schema_ids[&schema_metadata.name];
            for table_metadata in &schema_metadata.tables {
                let table_schema = effective_table_schema(schema_metadata, table_metadata);
                let table_key = (table_schema.to_string(), table_metadata.name.clone());
                if table_ids.contains_key(&table_key) {
                    return Err(CatalogBuildError::DuplicateTable {
                        schema: table_key.0,
                        table: table_key.1,
                    });
                }
                let table_id = TableId(tables.len());
                table_ids.insert(table_key, table_id);
                schemas[schema_id.0].tables.push(table_id);
                tables.push(Table::new(
                    table_id,
                    schema_id,
                    table_schema,
                    &table_metadata.name,
                    table_metadata.object_type,
                    table_metadata.description.clone(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                ));
                table_columns.push(Vec::new());
                table_primary_keys.push(Vec::new());
                table_unique_constraints.push(Vec::new());
                table_indexes.push(Vec::new());
            }
        }

        for schema_metadata in &metadata.schemas {
            for table_metadata in &schema_metadata.tables {
                let table_schema = effective_table_schema(schema_metadata, table_metadata);
                let table_id = table_ids[&(table_schema.to_string(), table_metadata.name.clone())];
                for column_metadata in &table_metadata.columns {
                    let key = (
                        table_schema.to_string(),
                        table_metadata.name.clone(),
                        column_metadata.name.clone(),
                    );
                    if column_ids.contains_key(&key) {
                        return Err(CatalogBuildError::DuplicateColumn {
                            schema: key.0,
                            table: key.1,
                            column: key.2,
                        });
                    }
                    let column_id = ColumnId(columns.len());
                    column_ids.insert(key, column_id);
                    table_columns[table_id.0].push(column_id);
                    columns.push(Column::new(
                        column_id,
                        table_id,
                        table_schema,
                        &table_metadata.name,
                        &column_metadata.name,
                        column_metadata.description.clone(),
                        type_ids[&column_metadata.provider_type],
                        column_metadata
                            .formatted_type
                            .clone()
                            .unwrap_or_else(|| column_metadata.data_type.as_str().to_string()),
                        column_metadata.type_modifier,
                        column_metadata.not_null,
                        false,
                    ));
                }
            }
        }

        for schema_metadata in &metadata.schemas {
            for table_metadata in &schema_metadata.tables {
                let table_schema = effective_table_schema(schema_metadata, table_metadata);
                let table_id = table_ids[&(table_schema.to_string(), table_metadata.name.clone())];

                for constraint in &table_metadata.constraints {
                    let constraint_name = constraint
                        .name
                        .as_deref()
                        .unwrap_or(constraint.kind.as_ref());
                    let constraint_columns = resolve_local_columns(
                        &column_ids,
                        table_schema,
                        &table_metadata.name,
                        constraint_name,
                        &constraint.columns,
                    )?;
                    match constraint.kind {
                        TableConstraintKind::PrimaryKey => {
                            table_primary_keys[table_id.0] = constraint_columns.clone();
                            push_unique_constraint(
                                &mut table_unique_constraints[table_id.0],
                                constraint_columns.clone(),
                            );
                        }
                        TableConstraintKind::Unique => {
                            push_unique_constraint(
                                &mut table_unique_constraints[table_id.0],
                                constraint_columns.clone(),
                            );
                        }
                    }
                }

                for index in &table_metadata.indexes {
                    let index_name = index.name.as_deref().unwrap_or("index");
                    let index_columns = resolve_local_columns(
                        &column_ids,
                        table_schema,
                        &table_metadata.name,
                        index_name,
                        &index
                            .keys
                            .iter()
                            .map(|key| key.column.clone())
                            .collect::<Vec<_>>(),
                    )?;
                    let included_columns = if index.included_columns.is_empty() {
                        Vec::new()
                    } else {
                        resolve_local_columns(
                            &column_ids,
                            table_schema,
                            &table_metadata.name,
                            index_name,
                            &index.included_columns,
                        )?
                    };
                    if index.unique {
                        push_unique_constraint(
                            &mut table_unique_constraints[table_id.0],
                            index_columns.clone(),
                        );
                        if index_columns.len() == 1 {
                            columns[index_columns[0].0].is_unique = true;
                        }
                    }
                    table_indexes[table_id.0].push(Index {
                        name: index.name.clone(),
                        access_method: index.access_method.clone(),
                        keys: index
                            .keys
                            .iter()
                            .zip(index_columns)
                            .map(|(key, column)| {
                                let mut capabilities = key.capabilities.clone();
                                capabilities.sort();
                                capabilities.dedup();
                                IndexKey {
                                    column,
                                    operator_class: key.operator_class.clone(),
                                    capabilities,
                                    order: key.order,
                                }
                            })
                            .collect(),
                        included_columns,
                        is_unique: index.unique,
                    });
                }
            }
        }

        let mut foreign_key_keys = HashSet::<(Vec<ColumnId>, Vec<ColumnId>)>::new();
        for schema_metadata in &metadata.schemas {
            for table_metadata in &schema_metadata.tables {
                let table_schema = effective_table_schema(schema_metadata, table_metadata);
                let from_table =
                    table_ids[&(table_schema.to_string(), table_metadata.name.clone())];
                for foreign_key in &table_metadata.foreign_keys {
                    if foreign_key.columns.len() != foreign_key.references.columns.len() {
                        return Err(CatalogBuildError::ForeignKeyColumnCountMismatch {
                            schema: table_schema.to_string(),
                            table: table_metadata.name.clone(),
                            name: foreign_key
                                .name
                                .clone()
                                .unwrap_or_else(|| "foreign_key".to_string()),
                            columns: foreign_key.columns.len(),
                            referenced_columns: foreign_key.references.columns.len(),
                        });
                    }
                    let foreign_key_name = foreign_key.name.as_deref().unwrap_or("foreign_key");
                    let from_columns = resolve_local_columns(
                        &column_ids,
                        table_schema,
                        &table_metadata.name,
                        foreign_key_name,
                        &foreign_key.columns,
                    )?;
                    let to_columns = foreign_key
                        .references
                        .columns
                        .iter()
                        .map(|column| {
                            column_ids
                                .get(&(
                                    foreign_key.references.schema.clone(),
                                    foreign_key.references.table.clone(),
                                    column.clone(),
                                ))
                                .copied()
                                .ok_or_else(|| CatalogBuildError::MissingForeignKeyTarget {
                                    schema: foreign_key.references.schema.clone(),
                                    table: foreign_key.references.table.clone(),
                                    column: column.clone(),
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let to_table = columns[to_columns[0].0].table;
                    add_foreign_key(
                        &mut foreign_keys,
                        &mut foreign_key_keys,
                        foreign_key.name.clone(),
                        from_columns,
                        to_columns,
                        from_table,
                        to_table,
                    );
                }
            }
        }

        for table in &mut tables {
            if !table_primary_keys[table.id.0].is_empty() {
                push_unique_constraint(
                    &mut table_unique_constraints[table.id.0],
                    table_primary_keys[table.id.0].clone(),
                );
            }
            table.columns = table_columns[table.id.0].clone();
            table.primary_key = table_primary_keys[table.id.0].clone();
            table.unique_constraints = table_unique_constraints[table.id.0].clone();
            table.indexes = table_indexes[table.id.0].clone();
        }

        let mut relations = Vec::with_capacity(foreign_keys.len() * 2);
        for foreign_key in &foreign_keys {
            let selector = foreign_key
                .from_columns
                .iter()
                .map(|column| columns[column.0].name.as_str())
                .collect::<Vec<_>>()
                .join("_");
            let forward = RelationId(relations.len());
            relations.push(Relation {
                id: forward,
                name: tables[foreign_key.to_table.0].name.clone(),
                selector: selector.clone(),
                visible: true,
                from_table: foreign_key.from_table,
                to_table: foreign_key.to_table,
                local_columns: foreign_key.from_columns.clone(),
                target_columns: foreign_key.to_columns.clone(),
                cardinality: RelationCardinality::Singular,
                nullable: foreign_key
                    .from_columns
                    .iter()
                    .any(|column| !columns[column.0].not_null),
                join_support: Some(foreign_key.id),
                join_direction: Some(ForeignKeyDirection::Referencing),
                supports: RelationSupports::default(),
            });
            tables[foreign_key.from_table.0].relations.push(forward);

            let reverse = RelationId(relations.len());
            let cardinality = if column_set_is_unique(
                &tables[foreign_key.from_table.0],
                &foreign_key.from_columns,
            ) {
                RelationCardinality::Singular
            } else {
                RelationCardinality::Collection
            };
            relations.push(Relation {
                id: reverse,
                name: tables[foreign_key.from_table.0].name.clone(),
                selector,
                visible: true,
                from_table: foreign_key.to_table,
                to_table: foreign_key.from_table,
                local_columns: foreign_key.to_columns.clone(),
                target_columns: foreign_key.from_columns.clone(),
                cardinality,
                nullable: true,
                join_support: Some(foreign_key.id),
                join_direction: Some(ForeignKeyDirection::Referenced),
                supports: RelationSupports::default(),
            });
            tables[foreign_key.to_table.0].relations.push(reverse);
        }

        Ok(Catalog {
            default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
            schemas,
            tables,
            types,
            type_ids,
            columns,
            foreign_keys,
            relations,
            uniqueness_supports: Vec::new(),
        })
    }
}

fn build_catalog_types(
    metadata: &DatabaseMetadata,
) -> Result<(Vec<CatalogType>, HashMap<TypeKey, TypeId>), CatalogBuildError> {
    let mut types = Vec::<CatalogType>::new();
    let mut type_ids = HashMap::<TypeKey, TypeId>::new();

    if metadata.types.is_empty() {
        for schema in &metadata.schemas {
            for table in &schema.tables {
                for column in &table.columns {
                    if let Some(type_id) = type_ids.get(&column.provider_type) {
                        let existing = types[type_id.0].data_type;
                        if existing != column.data_type {
                            return Err(CatalogBuildError::ConflictingFixtureType {
                                schema: column.provider_type.schema.clone(),
                                name: column.provider_type.name.clone(),
                                first: existing.as_str().to_string(),
                                second: column.data_type.as_str().to_string(),
                            });
                        }
                        continue;
                    }
                    let type_id = TypeId(types.len());
                    type_ids.insert(column.provider_type.clone(), type_id);
                    types.push(CatalogType::builtin(
                        type_id,
                        column.provider_type.clone(),
                        column.data_type,
                    ));
                }
            }
        }
        return Ok((types, type_ids));
    }

    for metadata_type in &metadata.types {
        let key = metadata_type.key();
        if type_ids.contains_key(&key) {
            return Err(CatalogBuildError::DuplicateType {
                schema: key.schema,
                name: key.name,
            });
        }
        let type_id = TypeId(types.len());
        type_ids.insert(key.clone(), type_id);
        types.push(CatalogType::builtin(type_id, key, DataType::Unknown));
    }

    let mut resolved = vec![None; metadata.types.len()];
    let mut states = vec![TypeResolutionState::Pending; metadata.types.len()];
    for index in 0..metadata.types.len() {
        resolve_catalog_type(
            index,
            &metadata.types,
            &type_ids,
            &mut states,
            &mut resolved,
        )?;
    }
    let types = resolved
        .into_iter()
        .enumerate()
        .map(|(index, data_type)| {
            data_type.ok_or_else(|| {
                let key = metadata.types[index].key();
                CatalogBuildError::InvalidTypeStructure {
                    schema: key.schema,
                    name: key.name,
                    message: "resolution produced no catalog type".to_string(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for schema in &metadata.schemas {
        for table in &schema.tables {
            let table_schema = effective_table_schema(schema, table);
            for column in &table.columns {
                let Some(type_id) = type_ids.get(&column.provider_type) else {
                    return Err(CatalogBuildError::MissingType(Box::new(
                        CatalogMissingType {
                            schema: table_schema.to_string(),
                            table: table.name.clone(),
                            column: column.name.clone(),
                            type_schema: column.provider_type.schema.clone(),
                            type_name: column.provider_type.name.clone(),
                        },
                    )));
                };
                let resolved = types[type_id.0].data_type;
                if resolved != column.data_type {
                    return Err(CatalogBuildError::TypeMismatch(Box::new(
                        CatalogTypeMismatch {
                            schema: table_schema.to_string(),
                            table: table.name.clone(),
                            column: column.name.clone(),
                            type_schema: column.provider_type.schema.clone(),
                            type_name: column.provider_type.name.clone(),
                            declared: column.data_type.as_str().to_string(),
                            resolved: resolved.as_str().to_string(),
                        },
                    )));
                }
            }
        }
    }

    Ok((types, type_ids))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeResolutionState {
    Pending,
    Resolving,
    Resolved,
}

fn resolve_catalog_type(
    index: usize,
    metadata: &[TypeMetadata],
    type_ids: &HashMap<TypeKey, TypeId>,
    states: &mut [TypeResolutionState],
    resolved: &mut [Option<CatalogType>],
) -> Result<(), CatalogBuildError> {
    let definition = &metadata[index];
    let key = definition.key();
    match states[index] {
        TypeResolutionState::Resolved => return Ok(()),
        TypeResolutionState::Resolving => {
            return Err(CatalogBuildError::CyclicTypeStructure {
                schema: key.schema,
                name: key.name,
            });
        }
        TypeResolutionState::Pending => {}
    }
    states[index] = TypeResolutionState::Resolving;
    let type_id = TypeId(index);
    let native_enum_provider = definition
        .provider
        .as_ref()
        .filter(|provider| provider.kind == "e");
    if native_enum_provider.is_some() && definition.structure.kind != TypeStructureKind::Enum {
        return Err(CatalogBuildError::StaleNativeEnum {
            schema: key.schema,
            name: key.name,
        });
    }

    let catalog_type = match definition.structure.kind {
        TypeStructureKind::Scalar => {
            if definition.structure.related_type.is_some() {
                return Err(invalid_type_structure(
                    &key,
                    "a scalar type cannot reference a related type",
                ));
            }
            if definition.structure.enumeration.is_some() {
                return Err(invalid_type_structure(
                    &key,
                    "a scalar type cannot carry enum metadata",
                ));
            }
            let data_type = DataType::from_database_type(&definition.internal_type);
            let capabilities = definition.provider.as_ref().map_or_else(
                || TypeCapabilities::builtin(data_type),
                |provider| {
                    TypeCapabilities::provider(
                        data_type,
                        &definition.readable_type,
                        provider,
                        &definition.operations,
                    )
                },
            );
            CatalogType {
                id: type_id,
                key: key.clone(),
                data_type,
                shape: CatalogTypeShape::Scalar,
                enumeration: None,
                readable_type: definition.readable_type.clone(),
                provider: definition.provider.clone(),
                capabilities,
            }
        }
        TypeStructureKind::Domain => {
            if definition.structure.enumeration.is_some() {
                return Err(invalid_type_structure(
                    &key,
                    "a domain type cannot carry enum metadata",
                ));
            }
            let base = related_type_id(definition, &key, type_ids, "base")?;
            resolve_catalog_type(base.0, metadata, type_ids, states, resolved)?;
            let Some(base_type) = resolved[base.0].as_ref() else {
                return Err(invalid_type_structure(&key, "base type did not resolve"));
            };
            CatalogType {
                id: type_id,
                key: key.clone(),
                data_type: base_type.data_type,
                shape: CatalogTypeShape::Domain { base },
                enumeration: None,
                readable_type: definition.readable_type.clone(),
                provider: definition.provider.clone(),
                capabilities: TypeCapabilities::domain(
                    &base_type.capabilities,
                    &definition.readable_type,
                    definition.provider.as_ref(),
                    &definition.operations,
                ),
            }
        }
        TypeStructureKind::Array => {
            if definition.structure.enumeration.is_some() {
                return Err(invalid_type_structure(
                    &key,
                    "an array type cannot carry enum metadata",
                ));
            }
            let element = related_type_id(definition, &key, type_ids, "element")?;
            resolve_catalog_type(element.0, metadata, type_ids, states, resolved)?;
            CatalogType {
                id: type_id,
                key: key.clone(),
                data_type: DataType::Unknown,
                shape: CatalogTypeShape::Array { element },
                enumeration: None,
                readable_type: definition.readable_type.clone(),
                provider: definition.provider.clone(),
                capabilities: TypeCapabilities::array(
                    &definition.readable_type,
                    definition.provider.as_ref(),
                    &definition.operations,
                ),
            }
        }
        TypeStructureKind::Enum => {
            if definition.structure.related_type.is_some() {
                return Err(invalid_type_structure(
                    &key,
                    "an enum type cannot reference a related type",
                ));
            }
            let Some(provider) = native_enum_provider else {
                return Err(invalid_type_structure(
                    &key,
                    "only provider-declared native enums are supported",
                ));
            };
            let Some(enumeration) = definition.structure.enumeration.as_ref() else {
                return Err(invalid_type_structure(
                    &key,
                    "a native enum requires enum metadata",
                ));
            };
            if enumeration.variants.is_empty() {
                return Err(invalid_type_structure(
                    &key,
                    "native enum types require at least one variant",
                ));
            }
            let mut variants = HashSet::new();
            let mut database_values = HashSet::new();
            for variant in &enumeration.variants {
                if !variants.insert(variant.variant.as_str()) {
                    return Err(invalid_type_structure(
                        &key,
                        &format!("duplicate enum variant {:?}", variant.variant),
                    ));
                }
                if !database_values.insert(variant.database_value.as_str()) {
                    return Err(invalid_type_structure(
                        &key,
                        &format!("duplicate enum database value {:?}", variant.database_value),
                    ));
                }
                if variant.variant != variant.database_value {
                    return Err(invalid_type_structure(
                        &key,
                        "native enum variants must equal their database values",
                    ));
                }
            }
            let data_type = DataType::Unknown;
            CatalogType {
                id: type_id,
                key: key.clone(),
                data_type,
                shape: CatalogTypeShape::Enum,
                enumeration: Some(CatalogEnum {
                    description: enumeration.description.clone(),
                    variants: enumeration
                        .variants
                        .iter()
                        .map(|variant| CatalogEnumVariant {
                            variant: variant.variant.clone(),
                            database_value: variant.database_value.clone(),
                            label: variant.label.clone(),
                            description: variant.description.clone(),
                        })
                        .collect(),
                }),
                readable_type: definition.readable_type.clone(),
                provider: definition.provider.clone(),
                capabilities: TypeCapabilities::provider(
                    data_type,
                    &definition.readable_type,
                    provider,
                    &definition.operations,
                ),
            }
        }
    };
    resolved[index] = Some(catalog_type);
    states[index] = TypeResolutionState::Resolved;
    Ok(())
}

fn related_type_id(
    definition: &TypeMetadata,
    key: &TypeKey,
    type_ids: &HashMap<TypeKey, TypeId>,
    relation: &'static str,
) -> Result<TypeId, CatalogBuildError> {
    let Some(related) = definition.structure.related_type.as_ref() else {
        return Err(invalid_type_structure(
            key,
            &format!(
                "a {} type requires a related type",
                definition.structure.kind.as_ref()
            ),
        ));
    };
    type_ids
        .get(related)
        .copied()
        .ok_or_else(|| CatalogBuildError::MissingRelatedType {
            schema: key.schema.clone(),
            name: key.name.clone(),
            relation,
            target_schema: related.schema.clone(),
            target_name: related.name.clone(),
        })
}

fn invalid_type_structure(key: &TypeKey, message: &str) -> CatalogBuildError {
    CatalogBuildError::InvalidTypeStructure {
        schema: key.schema.clone(),
        name: key.name.clone(),
        message: message.to_string(),
    }
}

fn column_set_is_unique(table: &Table, columns: &[ColumnId]) -> bool {
    table
        .unique_constraints
        .iter()
        .any(|constraint| column_set_covers(columns, constraint))
        || table.indexes.iter().any(|index| {
            index.is_unique
                && !index.keys.is_empty()
                && index.keys.iter().all(|key| columns.contains(&key.column))
        })
}

fn column_set_covers(columns: &[ColumnId], unique_columns: &[ColumnId]) -> bool {
    !unique_columns.is_empty()
        && unique_columns
            .iter()
            .all(|unique_column| columns.contains(unique_column))
}

fn effective_table_schema<'a>(schema: &'a SchemaMetadata, table: &'a TableMetadata) -> &'a str {
    if table.schema.is_empty() {
        &schema.name
    } else {
        &table.schema
    }
}

fn resolve_local_columns(
    column_ids: &HashMap<(String, String, String), ColumnId>,
    schema: &str,
    table: &str,
    name: &str,
    column_names: &[String],
) -> Result<Vec<ColumnId>, CatalogBuildError> {
    if column_names.is_empty() {
        return Err(CatalogBuildError::EmptyColumnSet {
            schema: schema.to_string(),
            table: table.to_string(),
            name: name.to_string(),
        });
    }
    column_names
        .iter()
        .map(|column| {
            column_ids
                .get(&(schema.to_string(), table.to_string(), column.clone()))
                .copied()
                .ok_or_else(|| CatalogBuildError::MissingColumn {
                    schema: schema.to_string(),
                    table: table.to_string(),
                    column: column.clone(),
                })
        })
        .collect()
}

fn push_unique_constraint(unique_constraints: &mut Vec<Vec<ColumnId>>, columns: Vec<ColumnId>) {
    if !unique_constraints
        .iter()
        .any(|existing| existing == &columns)
    {
        unique_constraints.push(columns);
    }
}

fn add_foreign_key(
    foreign_keys: &mut Vec<ForeignKey>,
    foreign_key_keys: &mut HashSet<(Vec<ColumnId>, Vec<ColumnId>)>,
    name: Option<String>,
    from_columns: Vec<ColumnId>,
    to_columns: Vec<ColumnId>,
    from_table: TableId,
    to_table: TableId,
) {
    if !foreign_key_keys.insert((from_columns.clone(), to_columns.clone())) {
        return;
    }
    let foreign_key_id = ForeignKeyId(foreign_keys.len());
    foreign_keys.push(ForeignKey {
        id: foreign_key_id,
        name,
        from_columns,
        to_columns,
        from_table,
        to_table,
    });
}

impl ObjectType {
    pub fn from_postgres_relkind(relkind: &str) -> Self {
        match relkind {
            "r" => Self::Table,
            "v" => Self::View,
            "m" => Self::MaterializedView,
            "i" => Self::Index,
            "S" => Self::Sequence,
            "s" => Self::Special,
            "t" => Self::ToastTable,
            "f" => Self::ForeignTable,
            _ => Self::Other,
        }
    }
}
