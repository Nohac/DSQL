use super::{
    Catalog, Column, ColumnId, DataType, ForeignKey, ForeignKeyId, Index, Schema, SchemaId, Table,
    TableId,
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
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct TypeMetadata {
    pub internal_type: String,
    pub readable_type: String,
    pub schema: String,
    pub operations: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
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

pub fn metadata_from_yaml(input: &str) -> Result<DatabaseMetadata, facet_yaml::DeserializeError> {
    facet_yaml::from_str(input)
}

pub fn metadata_to_yaml(metadata: &DatabaseMetadata) -> Result<String, String> {
    facet_yaml::to_string(metadata).map_err(|error| error.to_string())
}

pub fn table_metadata_from_yaml(
    input: &str,
) -> Result<TableMetadata, facet_yaml::DeserializeError> {
    facet_yaml::from_str(input)
}

pub fn table_metadata_to_yaml(table: &TableMetadata) -> Result<String, String> {
    facet_yaml::to_string(table).map_err(|error| error.to_string())
}

pub fn type_metadata_file_from_yaml(
    input: &str,
) -> Result<TypeMetadataFile, facet_yaml::DeserializeError> {
    facet_yaml::from_str(input)
}

pub fn type_metadata_file_to_yaml(types: &TypeMetadataFile) -> Result<String, String> {
    facet_yaml::to_string(types).map_err(|error| error.to_string())
}

impl DatabaseMetadata {
    pub fn into_catalog(self) -> Result<Catalog, CatalogBuildError> {
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
        self.types
            .sort_by(|left, right| left.internal_type.cmp(&right.internal_type));
    }
}

impl Catalog {
    pub fn try_from_metadata(metadata: DatabaseMetadata) -> Result<Self, CatalogBuildError> {
        let mut schemas = Vec::new();
        let mut tables = Vec::new();
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
                    table_metadata.description.clone(),
                    Vec::new(),
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
                        &column_metadata.database_type,
                        column_metadata.data_type,
                        column_metadata.not_null,
                        false,
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
                    for column_id in constraint_columns {
                        columns[column_id.0].is_indexed = true;
                    }
                }

                for index in &table_metadata.indexes {
                    let index_name = index.name.as_deref().unwrap_or("index");
                    let index_columns = resolve_local_columns(
                        &column_ids,
                        table_schema,
                        &table_metadata.name,
                        index_name,
                        &index.columns,
                    )?;
                    for column_id in &index_columns {
                        columns[column_id.0].is_indexed = true;
                    }
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
                        columns: index_columns,
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
                        &mut tables,
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

        Ok(Catalog {
            default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
            schemas,
            tables,
            columns,
            foreign_keys,
        })
    }
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

#[allow(clippy::too_many_arguments)]
fn add_foreign_key(
    foreign_keys: &mut Vec<ForeignKey>,
    tables: &mut [Table],
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
    tables[from_table.0].foreign_keys_from.push(foreign_key_id);
    tables[to_table.0].foreign_keys_to.push(foreign_key_id);
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
