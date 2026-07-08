//! Schema metadata loading: one YAML file per table, one directory per
//! schema, under the project's `schema/` directory.

use std::fs::{read_dir, read_to_string};
use std::path::Path;

use dsql_core::catalog::{DatabaseMetadata, SchemaMetadata, TableMetadata, table_metadata_from_yaml};

use super::config::{ProjectError, Result};

pub fn load_metadata_dir(path: &Path) -> Result<DatabaseMetadata> {
    let mut schemas = Vec::new();
    for entry in read_dir(path).map_err(|source| ProjectError::Read {
        path: path.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ProjectError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let schema_path = entry.path();
        if !schema_path.is_dir() {
            continue;
        }
        let Some(schema_name) = schema_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let mut tables = Vec::<TableMetadata>::new();
        for table_entry in read_dir(&schema_path).map_err(|source| ProjectError::Read {
            path: schema_path.clone(),
            source,
        })? {
            let table_path = table_entry
                .map_err(|source| ProjectError::Read {
                    path: schema_path.clone(),
                    source,
                })?
                .path();
            if table_path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let raw = read_to_string(&table_path).map_err(|source| ProjectError::Read {
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
            name: schema_name.to_string(),
            tables,
        });
    }
    schemas.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(DatabaseMetadata {
        schemas,
        types: Vec::new(),
    })
}
