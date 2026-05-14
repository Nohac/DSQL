use super::{
    Catalog, Column, ColumnId, DataType, ForeignKey, ForeignKeyId, Schema, SchemaId, Table, TableId,
};
use facet::Facet;
use std::collections::{BTreeSet, HashMap};
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
    pub columns: Vec<ColumnMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ColumnMetadata {
    pub name: String,
    pub database_type: String,
    pub data_type: DataType,
    pub not_null: bool,
    pub primary_key: bool,
    pub unique: bool,
    pub indexed: bool,
    #[facet(skip_serializing_if = Option::is_none)]
    pub foreign_key: Option<ForeignKeyMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ForeignKeyMetadata {
    pub schema: String,
    pub table: String,
    pub column: String,
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
                let table_schema = if table_metadata.schema.is_empty() {
                    schema_metadata.name.as_str()
                } else {
                    table_metadata.schema.as_str()
                };
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
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                ));
                table_columns.push(Vec::new());
                table_primary_keys.push(Vec::new());
            }
        }

        for schema_metadata in &metadata.schemas {
            for table_metadata in &schema_metadata.tables {
                let table_schema = if table_metadata.schema.is_empty() {
                    schema_metadata.name.as_str()
                } else {
                    table_metadata.schema.as_str()
                };
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
                    if column_metadata.primary_key {
                        table_primary_keys[table_id.0].push(column_id);
                    }
                    columns.push(Column::new(
                        column_id,
                        table_id,
                        table_schema,
                        &table_metadata.name,
                        &column_metadata.name,
                        column_metadata.data_type,
                        column_metadata.not_null,
                        column_metadata.unique,
                        column_metadata.indexed,
                    ));
                }
            }
        }

        for schema_metadata in &metadata.schemas {
            for table_metadata in &schema_metadata.tables {
                let table_schema = if table_metadata.schema.is_empty() {
                    schema_metadata.name.as_str()
                } else {
                    table_metadata.schema.as_str()
                };
                let from_table =
                    table_ids[&(table_schema.to_string(), table_metadata.name.clone())];
                for column_metadata in &table_metadata.columns {
                    let Some(target) = &column_metadata.foreign_key else {
                        continue;
                    };
                    let from_column = column_ids[&(
                        table_schema.to_string(),
                        table_metadata.name.clone(),
                        column_metadata.name.clone(),
                    )];
                    let Some(to_column) = column_ids
                        .get(&(
                            target.schema.clone(),
                            target.table.clone(),
                            target.column.clone(),
                        ))
                        .copied()
                    else {
                        return Err(CatalogBuildError::MissingForeignKeyTarget {
                            schema: target.schema.clone(),
                            table: target.table.clone(),
                            column: target.column.clone(),
                        });
                    };
                    let to_table = columns[to_column.0].table;
                    let foreign_key_id = ForeignKeyId(foreign_keys.len());
                    foreign_keys.push(ForeignKey {
                        id: foreign_key_id,
                        from_columns: vec![from_column],
                        to_columns: vec![to_column],
                        from_table,
                        to_table,
                    });
                    tables[from_table.0].foreign_keys_from.push(foreign_key_id);
                    tables[to_table.0].foreign_keys_to.push(foreign_key_id);
                }
            }
        }

        for table in &mut tables {
            table.columns = table_columns[table.id.0].clone();
            table.primary_key = table_primary_keys[table.id.0].clone();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_metadata_yaml_round_trips_columns() {
        let table = TableMetadata {
            schema: "public".to_string(),
            name: "posts".to_string(),
            object_type: ObjectType::Table,
            columns: vec![
                ColumnMetadata {
                    name: "id".to_string(),
                    database_type: "int4".to_string(),
                    data_type: DataType::Int,
                    not_null: true,
                    primary_key: true,
                    unique: true,
                    indexed: true,
                    foreign_key: None,
                },
                ColumnMetadata {
                    name: "user_id".to_string(),
                    database_type: "int4".to_string(),
                    data_type: DataType::Int,
                    not_null: true,
                    primary_key: false,
                    unique: false,
                    indexed: true,
                    foreign_key: Some(ForeignKeyMetadata {
                        schema: "public".to_string(),
                        table: "users".to_string(),
                        column: "id".to_string(),
                    }),
                },
            ],
        };

        let yaml = table_metadata_to_yaml(&table).expect("table metadata should serialize");
        assert!(yaml.contains("columns:"));
        let decoded =
            table_metadata_from_yaml(&yaml).expect("table metadata yaml should deserialize");
        assert_eq!(decoded, table);
    }
}
