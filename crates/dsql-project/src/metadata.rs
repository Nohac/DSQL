//! Schema metadata storage: one YAML file per table, one directory per
//! schema, plus `type_map.yaml`, under the project's `schema/` directory.
//! Loading and storing round-trip so introspection output is diffable.

use std::collections::BTreeSet;
use std::path::Path;

use tokio::fs::{create_dir_all, read_dir, read_to_string, remove_file, write};

use dsql_core::catalog::{
    DatabaseMetadata, SchemaMetadata, TableMetadata, TypeMetadataFile, table_metadata_from_yaml,
    table_metadata_to_yaml, type_metadata_file_from_yaml, type_metadata_file_to_yaml,
};

use super::config::{ProjectError, Result};

/// Collects a directory's entry paths, mapping errors onto `dir`.
async fn entry_paths(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut entries = read_dir(dir).await.map_err(|source| ProjectError::Read {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut paths = Vec::new();
    loop {
        let entry = entries
            .next_entry()
            .await
            .map_err(|source| ProjectError::Read {
                path: dir.to_path_buf(),
                source,
            })?;
        let Some(entry) = entry else {
            return Ok(paths);
        };
        paths.push(entry.path());
    }
}

pub async fn load_metadata_dir(path: &Path) -> Result<DatabaseMetadata> {
    let mut schemas = Vec::new();
    for schema_path in entry_paths(path).await? {
        if !schema_path.is_dir() {
            continue;
        }
        let Some(schema_name) = schema_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let schema_name = schema_name.to_string();
        let mut tables = Vec::<TableMetadata>::new();
        for table_path in entry_paths(&schema_path).await? {
            if table_path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let raw = read_to_string(&table_path)
                .await
                .map_err(|source| ProjectError::Read {
                    path: table_path.clone(),
                    source,
                })?;
            let table = table_metadata_from_yaml(&raw).map_err(|error| ProjectError::Parse {
                path: table_path.clone(),
                message: error.to_string(),
            })?;
            tables.push(table);
        }
        tables.sort_by(|left, right| left.name.cmp(&right.name));
        schemas.push(SchemaMetadata {
            name: schema_name,
            tables,
        });
    }
    schemas.sort_by(|left, right| left.name.cmp(&right.name));

    let types_path = path.join("type_map.yaml");
    let types = match read_to_string(&types_path).await {
        Ok(raw) => {
            type_metadata_file_from_yaml(&raw)
                .map_err(|error| ProjectError::Parse {
                    path: types_path,
                    message: error.to_string(),
                })?
                .types
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => {
            return Err(ProjectError::Read {
                path: types_path,
                source,
            });
        }
    };

    Ok(DatabaseMetadata { schemas, types })
}

/// Writes `metadata` as the schema directory's contents, removing table
/// files for tables that no longer exist.
pub async fn store_metadata_dir(metadata: &DatabaseMetadata, path: &Path) -> Result<()> {
    let mut metadata = metadata.clone();
    metadata.canonicalize();
    create_dir(path).await?;
    for schema in &metadata.schemas {
        let schema_path = path.join(&schema.name);
        create_dir(&schema_path).await?;
        let mut expected_tables = BTreeSet::new();
        for table in &schema.tables {
            let table_file = format!("{}.yaml", table.name);
            expected_tables.insert(table_file.clone());
            let table_yaml =
                table_metadata_to_yaml(table).map_err(|message| ProjectError::Parse {
                    path: schema_path.join(&table_file),
                    message,
                })?;
            write_file(&schema_path.join(table_file), &table_yaml).await?;
        }
        remove_stale_table_files(&schema_path, &expected_tables).await?;
    }
    let types_path = path.join("type_map.yaml");
    let types_yaml = type_metadata_file_to_yaml(&TypeMetadataFile {
        types: metadata.types.clone(),
    })
    .map_err(|message| ProjectError::Parse {
        path: types_path.clone(),
        message,
    })?;
    write_file(&types_path, &types_yaml).await
}

async fn create_dir(path: &Path) -> Result<()> {
    create_dir_all(path)
        .await
        .map_err(|source| ProjectError::Write {
            path: path.to_path_buf(),
            source,
        })
}

async fn write_file(path: &Path, content: &str) -> Result<()> {
    write(path, content)
        .await
        .map_err(|source| ProjectError::Write {
            path: path.to_path_buf(),
            source,
        })
}

async fn remove_stale_table_files(
    schema_path: &Path,
    expected_tables: &BTreeSet<String>,
) -> Result<()> {
    for path in entry_paths(schema_path).await? {
        let is_stale_yaml = path.extension().and_then(|ext| ext.to_str()) == Some("yaml")
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !expected_tables.contains(name));
        if is_stale_yaml {
            remove_file(&path)
                .await
                .map_err(|source| ProjectError::Write {
                    path: path.clone(),
                    source,
                })?;
        }
    }
    Ok(())
}
