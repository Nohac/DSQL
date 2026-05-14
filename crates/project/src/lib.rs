use dsql_core::{
    Catalog, DatabaseMetadata, SchemaMetadata, TableMetadata, TypeMetadataFile,
    table_metadata_from_yaml, table_metadata_to_yaml, type_metadata_file_from_yaml,
    type_metadata_file_to_yaml,
};
use facet::Facet;
use miette::{IntoDiagnostic, Result, miette};
use std::{
    collections::BTreeSet,
    env::current_dir,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Facet)]
pub struct Config {
    pub database_url: String,
    #[facet(default = default_schema())]
    pub default_schema: String,
    pub documents: Vec<DocumentConfig>,
}

#[derive(Clone, Debug, Facet)]
pub struct DocumentConfig {
    pub resolver: ResolverType,
    pub paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum ResolverType {
    Dsql,
    Typescript,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub schema: PathBuf,
    pub config: Config,
}

impl Project {
    pub fn load() -> Result<Self> {
        let start = current_dir().into_diagnostic()?;
        Self::load_from(&start)
    }

    pub fn load_from(start_dir: &Path) -> Result<Self> {
        let root = find_root(start_dir).ok_or_else(|| {
            miette!(
                help = "try running: dsql init",
                "failed to locate a valid dsql project"
            )
        })?;
        let config_path = root.join("dsql.toml");
        let config: Config =
            facet_toml::from_str(&fs::read_to_string(&config_path).into_diagnostic()?)
                .map_err(|error| miette!("failed to parse {}: {error}", config_path.display()))?;
        Ok(Self {
            schema: root.join("schema"),
            config,
        })
    }

    pub fn try_load() -> Option<Self> {
        Self::load().ok()
    }

    pub fn try_load_from(start_dir: &Path) -> Option<Self> {
        Self::load_from(start_dir).ok()
    }

    pub fn load_catalog(&self) -> Result<Catalog> {
        let metadata = load_metadata_dir(&self.schema)?;
        metadata
            .into_catalog()
            .map(|catalog| catalog.with_default_schema(self.config.default_schema.clone()))
            .map_err(|error| miette!("failed to build catalog from schema metadata: {error}"))
    }
}

fn default_schema() -> String {
    Catalog::DEFAULT_SCHEMA.to_string()
}

pub fn find_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current_dir = start_dir;
    loop {
        let candidate = current_dir.join("dsql");
        if candidate.join("dsql.toml").exists() {
            return Some(candidate);
        }
        current_dir = current_dir.parent()?;
    }
}

pub fn init_project(base_path: &Path, database_url: Option<String>) -> Result<Project> {
    let root = base_path.join("dsql");
    let schema = root.join("schema");
    fs::create_dir_all(&schema).into_diagnostic()?;
    let config = Config {
        database_url: database_url.unwrap_or_else(|| "<database url>".to_string()),
        default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
        documents: Vec::new(),
    };
    let config_toml = facet_toml::to_string(&config)
        .map_err(|error| miette!("failed to serialize project config: {error}"))?;
    fs::write(root.join("dsql.toml"), config_toml).into_diagnostic()?;
    Ok(Project { schema, config })
}

pub fn load_metadata_dir(path: &Path) -> Result<DatabaseMetadata> {
    let mut schemas = Vec::new();
    for entry in fs::read_dir(path).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        let schema_path = entry.path();
        if !schema_path.is_dir() {
            continue;
        }
        let Some(schema_name) = schema_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let mut tables = Vec::<TableMetadata>::new();
        for table_entry in fs::read_dir(&schema_path).into_diagnostic()? {
            let table_entry = table_entry.into_diagnostic()?;
            let table_path = table_entry.path();
            if table_path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let table =
                table_metadata_from_yaml(&fs::read_to_string(&table_path).into_diagnostic()?)
                    .map_err(|error| {
                        miette!("failed to parse {}: {error}", table_path.display())
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

    let types_path = path.join("type_map.yaml");
    let types = if types_path.exists() {
        type_metadata_file_from_yaml(&fs::read_to_string(&types_path).into_diagnostic()?)
            .map_err(|error| miette!("failed to parse {}: {error}", types_path.display()))?
            .types
    } else {
        Vec::new()
    };

    Ok(DatabaseMetadata { schemas, types })
}

pub fn store_metadata_dir(metadata: &DatabaseMetadata, path: &Path) -> Result<()> {
    let mut metadata = metadata.clone();
    metadata.canonicalize();
    fs::create_dir_all(path).into_diagnostic()?;
    for schema in &metadata.schemas {
        let schema_path = path.join(&schema.name);
        fs::create_dir_all(&schema_path).into_diagnostic()?;
        let mut expected_tables = BTreeSet::new();
        for table in &schema.tables {
            let table_file = format!("{}.yaml", table.name);
            expected_tables.insert(table_file.clone());
            let table_yaml = table_metadata_to_yaml(table)
                .map_err(|error| miette!("failed to serialize table metadata: {error}"))?;
            fs::write(schema_path.join(table_file), table_yaml).into_diagnostic()?;
        }
        remove_stale_table_files(&schema_path, &expected_tables)?;
    }
    let types_yaml = type_metadata_file_to_yaml(&TypeMetadataFile {
        types: metadata.types.clone(),
    })
    .map_err(|error| miette!("failed to serialize type metadata: {error}"))?;
    fs::write(path.join("type_map.yaml"), types_yaml).into_diagnostic()?;
    let stale_type_map = path.join("type_map.toml");
    if stale_type_map.exists() {
        fs::remove_file(stale_type_map).into_diagnostic()?;
    }
    Ok(())
}

fn remove_stale_table_files(schema_path: &Path, expected_tables: &BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(schema_path).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str());
        if extension != Some("yaml") && extension != Some("toml") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !expected_tables.contains(file_name) {
            fs::remove_file(path).into_diagnostic()?;
        }
    }
    Ok(())
}
