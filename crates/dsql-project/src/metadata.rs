//! Schema metadata storage: one YAML file per table, one directory per
//! schema, plus `type_map.yaml`, under the project's `schema/` directory.
//! Loading and storing round-trip so introspection output is diffable.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::fs::{create_dir_all, read_dir, read_to_string, remove_dir_all, rename, write};

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

/// Loads one complete generated catalog.
///
/// If publication was interrupted between the two directory renames, loading
/// restores the stable backup first. Incomplete staging directories are inert
/// siblings and are intentionally never read or removed here.
pub async fn load_metadata_dir(path: &Path) -> Result<DatabaseMetadata> {
    recover_metadata_dir(path).await?;
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

static NEXT_PUBLICATION: AtomicU64 = AtomicU64::new(0);

/// Transactionally replaces the complete generated schema directory.
pub async fn store_metadata_dir(metadata: &DatabaseMetadata, path: &Path) -> Result<()> {
    store_metadata_dir_inner(metadata, path, false).await
}

async fn store_metadata_dir_inner(
    metadata: &DatabaseMetadata,
    path: &Path,
    inject_promotion_failure: bool,
) -> Result<()> {
    let mut metadata = metadata.clone();
    metadata.canonicalize();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_dir(parent).await?;
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("schema");
    let publication = NEXT_PUBLICATION.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(
        ".{stem}.stage-{}-{publication}",
        std::process::id()
    ));
    let backup = parent.join(format!(".{stem}.backup"));
    recover_metadata_dir(path).await?;
    if path_exists(&backup).await? {
        remove_dir_all(&backup)
            .await
            .map_err(|source| ProjectError::Write {
                path: backup.clone(),
                source,
            })?;
    }
    if path_exists(&staging).await? {
        remove_dir_all(&staging)
            .await
            .map_err(|source| ProjectError::Write {
                path: staging.clone(),
                source,
            })?;
    }
    if let Err(error) = write_metadata_dir_contents(&metadata, &staging).await {
        let _ = remove_dir_all(&staging).await;
        return Err(error);
    }
    if path_exists(path).await? {
        if let Err(source) = rename(path, &backup).await {
            let _ = remove_dir_all(&staging).await;
            return Err(ProjectError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        let promoted = if inject_promotion_failure {
            Err(std::io::Error::other("injected schema promotion failure"))
        } else {
            rename(&staging, path).await
        };
        if let Err(source) = promoted {
            let _ = rename(&backup, path).await;
            let _ = remove_dir_all(&staging).await;
            return Err(ProjectError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        let _ = remove_dir_all(&backup).await;
    } else {
        if let Err(source) = rename(&staging, path).await {
            let _ = remove_dir_all(&staging).await;
            return Err(ProjectError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    Ok(())
}

/// Restores the last complete generated catalog after an interrupted
/// publication left the live directory in its rename window.
async fn recover_metadata_dir(path: &Path) -> Result<()> {
    if path_exists(path).await? {
        return Ok(());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("schema");
    let backup = parent.join(format!(".{stem}.backup"));
    if path_exists(&backup).await? {
        rename(&backup, path)
            .await
            .map_err(|source| ProjectError::Write {
                path: path.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}

async fn path_exists(path: &Path) -> Result<bool> {
    tokio::fs::try_exists(path)
        .await
        .map_err(|source| ProjectError::Read {
            path: path.to_path_buf(),
            source,
        })
}

async fn write_metadata_dir_contents(metadata: &DatabaseMetadata, path: &Path) -> Result<()> {
    create_dir(path).await?;
    for schema in &metadata.schemas {
        let schema_path = path.join(&schema.name);
        create_dir(&schema_path).await?;
        for table in &schema.tables {
            let table_file = format!("{}.yaml", table.name);
            let table_yaml =
                table_metadata_to_yaml(table).map_err(|message| ProjectError::Parse {
                    path: schema_path.join(&table_file),
                    message,
                })?;
            write_file(&schema_path.join(table_file), &table_yaml).await?;
        }
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

#[cfg(test)]
mod tests {
    use dsql_core::catalog::{ObjectType, TableMetadata};

    use super::*;

    fn metadata_with_table(name: &str) -> DatabaseMetadata {
        DatabaseMetadata {
            schemas: vec![SchemaMetadata {
                name: "public".to_string(),
                tables: vec![TableMetadata {
                    schema: "public".to_string(),
                    name: name.to_string(),
                    object_type: ObjectType::Table,
                    description: None,
                    columns: Vec::new(),
                    constraints: Vec::new(),
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                }],
            }],
            types: Vec::new(),
        }
    }

    #[tokio::test]
    async fn promotion_failure_restores_the_complete_live_generation() {
        let scratch = tempfile::tempdir().expect("scratch directory");
        let schema = scratch.path().join("schema");
        let previous = metadata_with_table("previous");
        store_metadata_dir(&previous, &schema)
            .await
            .expect("previous generation publishes");

        let error = store_metadata_dir_inner(&metadata_with_table("candidate"), &schema, true)
            .await
            .expect_err("promotion failure is injected after staging");
        assert!(
            error
                .to_string()
                .contains("injected schema promotion failure")
        );

        let restored = load_metadata_dir(&schema)
            .await
            .expect("previous generation remains readable");
        let mut previous = previous;
        previous.canonicalize();
        assert_eq!(restored, previous);
        assert!(schema.join("public/previous.yaml").exists());
        assert!(
            !schema.join("public/candidate.yaml").exists(),
            "no candidate content reaches the restored live directory"
        );
        assert!(
            !scratch.path().join(".schema.backup").exists(),
            "successful rollback consumes the stable backup"
        );
    }
}
