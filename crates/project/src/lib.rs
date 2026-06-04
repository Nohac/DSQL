use dsql_core::{
    Catalog, CatalogBuildError, DatabaseMetadata, LintOptions, SchemaMetadata, Severity,
    TableMetadata, TypeMetadataFile, table_metadata_from_yaml, table_metadata_to_yaml,
    type_metadata_file_from_yaml, type_metadata_file_to_yaml,
};
use dsql_embedding::{EmbeddedRegion, RegexEmbedding, default_typescript_regex_pattern};
use facet::Facet;
use std::{
    collections::{BTreeMap, BTreeSet},
    env::current_dir,
    fs, io,
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, ProjectError>;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("failed to locate a valid dsql project")]
    MissingRoot,
    #[error("failed to read current directory: {0}")]
    CurrentDir(#[source] io::Error),
    #[error("failed to read {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("failed to create directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("failed to read directory {path}: {source}")]
    ReadDir { path: PathBuf, source: io::Error },
    #[error("failed to read directory entry in {path}: {source}")]
    ReadDirEntry { path: PathBuf, source: io::Error },
    #[error("failed to remove {path}: {source}")]
    RemoveFile { path: PathBuf, source: io::Error },
    #[error("failed to parse {path}: {message}")]
    ParseFile { path: PathBuf, message: String },
    #[error("failed to serialize project config: {0}")]
    SerializeConfig(String),
    #[error("failed to serialize table metadata: {0}")]
    SerializeTableMetadata(String),
    #[error("failed to serialize type metadata: {0}")]
    SerializeTypeMetadata(String),
    #[error("failed to build catalog from schema metadata: {0}")]
    CatalogBuild(#[from] CatalogBuildError),
    #[error("document path not found: {0}")]
    DocumentPathNotFound(PathBuf),
    #[error("invalid document glob `{pattern}`: {message}")]
    InvalidDocumentGlob { pattern: String, message: String },
    #[error("failed to read document glob entry: {0}")]
    DocumentGlobEntry(String),
    #[error("embedding `{resolver}` with strategy `regex` requires `pattern`")]
    MissingEmbeddingPattern { resolver: String },
    #[error("document resolver `{resolver}` requires an [embedding.{resolver}] config")]
    MissingEmbeddingConfig { resolver: String },
    #[error("failed to extract embedded DSQL from {path}: {source}")]
    EmbeddedExtraction {
        path: PathBuf,
        source: dsql_embedding::EmbeddingError,
    },
}

impl miette::Diagnostic for ProjectError {
    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match self {
            ProjectError::MissingRoot => Some(Box::new("try running: dsql init")),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Facet)]
pub struct Config {
    pub database_url: String,
    #[facet(default = default_schema())]
    pub default_schema: String,
    #[facet(default = default_lint_config())]
    pub lint: LintConfig,
    #[facet(default = default_generate_config())]
    pub generate: GenerateConfig,
    #[facet(default)]
    pub embedding: BTreeMap<String, EmbeddingConfig>,
    pub documents: Vec<DocumentConfig>,
}

#[derive(Clone, Debug, Facet)]
pub struct LintConfig {
    #[facet(default = default_unindexed_scan_severity())]
    pub unindexed_scan_severity: LintSeverity,
}

#[derive(Clone, Debug, Facet)]
pub struct GenerateConfig {
    #[facet(default = default_typescript_generate_config())]
    pub typescript: TypescriptGenerateConfig,
}

#[derive(Clone, Debug, Facet)]
pub struct TypescriptGenerateConfig {
    #[facet(default = default_false())]
    pub enabled: bool,
    #[facet(default = default_typescript_out_dir())]
    pub out_dir: String,
    #[facet(default)]
    pub cmd: Vec<String>,
}

#[derive(Clone, Copy, Debug, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum LintSeverity {
    Off,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Facet)]
pub struct DocumentConfig {
    pub resolver: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Facet)]
pub struct EmbeddingConfig {
    pub strategy: EmbeddingStrategy,
    #[facet(default)]
    pub pattern: Option<String>,
    #[facet(default)]
    pub language: Option<String>,
    #[facet(default)]
    pub query: Option<String>,
}

#[derive(Clone, Copy, Debug, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum EmbeddingStrategy {
    Regex,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub schema: PathBuf,
    pub config: Config,
}

impl Project {
    pub fn load() -> Result<Self> {
        let start = current_dir().map_err(ProjectError::CurrentDir)?;
        Self::load_from(&start)
    }

    pub fn load_from(start_dir: &Path) -> Result<Self> {
        let root = find_root(start_dir).ok_or(ProjectError::MissingRoot)?;
        let config_path = root.join("dsql.toml");
        let config: Config =
            facet_toml::from_str(&read_to_string(&config_path)?).map_err(|error| {
                ProjectError::ParseFile {
                    path: config_path.clone(),
                    message: error.to_string(),
                }
            })?;
        Ok(Self {
            schema: root.join("schema"),
            root,
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
            .map_err(ProjectError::CatalogBuild)
    }

    pub fn lint_options(&self) -> LintOptions {
        LintOptions {
            unindexed_scan_severity: match self.config.lint.unindexed_scan_severity {
                LintSeverity::Off => None,
                LintSeverity::Info => Some(Severity::Info),
                LintSeverity::Warning => Some(Severity::Warning),
                LintSeverity::Error => Some(Severity::Error),
            },
        }
    }
}

fn default_schema() -> String {
    Catalog::DEFAULT_SCHEMA.to_string()
}

fn default_lint_config() -> LintConfig {
    LintConfig {
        unindexed_scan_severity: default_unindexed_scan_severity(),
    }
}

fn default_generate_config() -> GenerateConfig {
    GenerateConfig {
        typescript: default_typescript_generate_config(),
    }
}

fn default_typescript_generate_config() -> TypescriptGenerateConfig {
    TypescriptGenerateConfig {
        enabled: false,
        out_dir: default_typescript_out_dir(),
        cmd: Vec::new(),
    }
}

fn default_false() -> bool {
    false
}

fn default_typescript_out_dir() -> String {
    "src/generated/dsql".to_string()
}

fn default_unindexed_scan_severity() -> LintSeverity {
    LintSeverity::Info
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
    create_dir_all(&schema)?;
    let config = Config {
        database_url: database_url.unwrap_or_else(|| "<database url>".to_string()),
        default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
        lint: default_lint_config(),
        generate: default_generate_config(),
        embedding: BTreeMap::new(),
        documents: Vec::new(),
    };
    let config_toml = facet_toml::to_string(&config)
        .map_err(|error| ProjectError::SerializeConfig(error.to_string()))?;
    write_file(&root.join("dsql.toml"), config_toml)?;
    Ok(Project {
        root,
        schema,
        config,
    })
}

pub fn load_metadata_dir(path: &Path) -> Result<DatabaseMetadata> {
    let mut schemas = Vec::new();
    for entry in read_dir(path)? {
        let entry = entry.map_err(|source| ProjectError::ReadDirEntry {
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
        for table_entry in read_dir(&schema_path)? {
            let table_entry = table_entry.map_err(|source| ProjectError::ReadDirEntry {
                path: schema_path.clone(),
                source,
            })?;
            let table_path = table_entry.path();
            if table_path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let table =
                table_metadata_from_yaml(&read_to_string(&table_path)?).map_err(|error| {
                    ProjectError::ParseFile {
                        path: table_path.clone(),
                        message: error.to_string(),
                    }
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
        type_metadata_file_from_yaml(&read_to_string(&types_path)?)
            .map_err(|error| ProjectError::ParseFile {
                path: types_path.clone(),
                message: error.to_string(),
            })?
            .types
    } else {
        Vec::new()
    };

    Ok(DatabaseMetadata { schemas, types })
}

pub fn store_metadata_dir(metadata: &DatabaseMetadata, path: &Path) -> Result<()> {
    let mut metadata = metadata.clone();
    metadata.canonicalize();
    create_dir_all(path)?;
    for schema in &metadata.schemas {
        let schema_path = path.join(&schema.name);
        create_dir_all(&schema_path)?;
        let mut expected_tables = BTreeSet::new();
        for table in &schema.tables {
            let table_file = format!("{}.yaml", table.name);
            expected_tables.insert(table_file.clone());
            let table_yaml = table_metadata_to_yaml(table)
                .map_err(|error| ProjectError::SerializeTableMetadata(error.to_string()))?;
            write_file(&schema_path.join(table_file), table_yaml)?;
        }
        remove_stale_table_files(&schema_path, &expected_tables)?;
    }
    let types_yaml = type_metadata_file_to_yaml(&TypeMetadataFile {
        types: metadata.types.clone(),
    })
    .map_err(|error| ProjectError::SerializeTypeMetadata(error.to_string()))?;
    write_file(&path.join("type_map.yaml"), types_yaml)?;
    let stale_type_map = path.join("type_map.toml");
    if stale_type_map.exists() {
        remove_file(&stale_type_map)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
    pub source_offset: usize,
}

pub fn load_project_documents(project: &Project) -> Result<Vec<ProjectDocument>> {
    let base = project_base(project);
    let mut documents = Vec::new();
    if project.config.documents.is_empty() {
        let mut files = Vec::new();
        collect_dsql_files(&base, Some(&project.root), &mut files)?;
        files.sort();
        files.dedup();
        for path in files {
            documents.push(read_dsql_document(path)?);
        }
    } else {
        for document_config in &project.config.documents {
            let mut files = Vec::new();
            for path in &document_config.paths {
                collect_resolver_path(&base.join(path), Some(&project.root), &mut files)?;
            }
            files.sort();
            files.dedup();

            if document_config.resolver == "dsql" {
                for path in files {
                    if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
                        documents.push(read_dsql_document(path)?);
                    }
                }
            } else {
                let embedding = embedding_for_resolver(project, &document_config.resolver)?;
                for path in files {
                    documents.extend(read_embedded_documents(path, &embedding)?);
                }
            }
        }
    }
    documents.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.source_offset.cmp(&right.source_offset))
    });
    Ok(documents)
}

pub fn project_base(project: &Project) -> PathBuf {
    project
        .root
        .parent()
        .map_or_else(|| project.root.clone(), Path::to_path_buf)
}

fn read_dsql_document(path: PathBuf) -> Result<ProjectDocument> {
    let text = read_to_string(&path)?;
    Ok(ProjectDocument {
        path,
        text,
        source_offset: 0,
    })
}

fn read_embedded_documents(
    path: PathBuf,
    embedding: &RegexEmbedding,
) -> Result<Vec<ProjectDocument>> {
    let source = read_to_string(&path)?;
    embedding
        .extract(&source)
        .map_err(|source| ProjectError::EmbeddedExtraction {
            path: path.clone(),
            source,
        })?
        .into_iter()
        .map(|region| embedded_document(&path, region))
        .collect()
}

fn embedded_document(path: &Path, region: EmbeddedRegion) -> Result<ProjectDocument> {
    Ok(ProjectDocument {
        path: path.to_path_buf(),
        text: region.text,
        source_offset: region.content_range.start as usize,
    })
}

fn embedding_for_resolver(project: &Project, resolver: &str) -> Result<RegexEmbedding> {
    let pattern = if let Some(config) = project.config.embedding.get(resolver) {
        match config.strategy {
            EmbeddingStrategy::Regex => {
                config
                    .pattern
                    .clone()
                    .ok_or_else(|| ProjectError::MissingEmbeddingPattern {
                        resolver: resolver.to_string(),
                    })?
            }
        }
    } else if resolver == "typescript" {
        default_typescript_regex_pattern()
    } else {
        return Err(ProjectError::MissingEmbeddingConfig {
            resolver: resolver.to_string(),
        });
    };
    Ok(RegexEmbedding::new(pattern))
}

fn collect_resolver_path(
    path: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if path_has_glob(path) {
        return collect_glob_path(path, excluded_dir, files);
    }
    if path.is_dir() {
        collect_all_files(path, excluded_dir, files)
    } else if path.is_file() {
        files.push(path.to_path_buf());
        Ok(())
    } else {
        Err(ProjectError::DocumentPathNotFound(path.to_path_buf()))
    }
}

fn collect_glob_path(
    path: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let pattern = path.to_string_lossy().to_string();
    for entry in glob::glob(&pattern).map_err(|error| ProjectError::InvalidDocumentGlob {
        pattern: pattern.clone(),
        message: error.to_string(),
    })? {
        let path = entry.map_err(|error| ProjectError::DocumentGlobEntry(error.to_string()))?;
        if excluded_dir.is_some_and(|excluded| path.starts_with(excluded)) {
            continue;
        }
        if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn path_has_glob(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|char| matches!(char, '*' | '?' | '[' | ']'))
}

fn collect_all_files(
    dir: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if excluded_dir.is_some_and(|excluded| dir == excluded) {
        return Ok(());
    }
    for entry in read_dir(dir)? {
        let entry = entry.map_err(|source| ProjectError::ReadDirEntry {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_all_files(&path, excluded_dir, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_dsql_files(
    dir: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if excluded_dir.is_some_and(|excluded| dir == excluded) {
        return Ok(());
    }
    for entry in read_dir(dir)? {
        let entry = entry.map_err(|source| ProjectError::ReadDirEntry {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_dsql_files(&path, excluded_dir, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path);
        }
    }
    Ok(())
}

fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| ProjectError::ReadFile {
        path: path.to_path_buf(),
        source,
    })
}

fn write_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, contents).map_err(|source| ProjectError::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}

fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| ProjectError::CreateDir {
        path: path.to_path_buf(),
        source,
    })
}

fn read_dir(path: &Path) -> Result<fs::ReadDir> {
    fs::read_dir(path).map_err(|source| ProjectError::ReadDir {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_file(path: &Path) -> Result<()> {
    fs::remove_file(path).map_err(|source| ProjectError::RemoveFile {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn load_from_reports_missing_root_as_typed_error() {
        let root = temp_root("missing-root");
        fs::create_dir_all(&root).unwrap();

        let error = Project::load_from(&root).unwrap_err();

        assert!(matches!(error, ProjectError::MissingRoot));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_from_reports_invalid_config_as_typed_error() {
        let root = temp_root("invalid-config");
        fs::create_dir_all(root.join("dsql")).unwrap();
        fs::write(root.join("dsql/dsql.toml"), "database_url = [").unwrap();

        let error = Project::load_from(&root).unwrap_err();

        assert!(matches!(error, ProjectError::ParseFile { .. }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_documents_load_embedded_typescript_regions() {
        let root = temp_root("embedded-documents");
        fs::create_dir_all(root.join("dsql")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("dsql/dsql.toml"),
            r#"database_url = "<database url>"
documents = [{ resolver = "typescript", paths = ["src/**/*.ts"] }]
"#,
        )
        .unwrap();
        let source = r#"const query = dsql(`
query Users { users { id } }
`);
"#;
        fs::write(root.join("src/query.ts"), source).unwrap();

        let project = Project::load_from(&root).unwrap();
        let documents = load_project_documents(&project).unwrap();

        assert_eq!(documents.len(), 1);
        assert_eq!(
            documents[0].source_offset,
            source.find(&documents[0].text).unwrap()
        );
        assert!(documents[0].text.contains("query Users"));
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "dsql-project-{name}-{}-{unique}",
            std::process::id()
        ))
    }
}

fn remove_stale_table_files(schema_path: &Path, expected_tables: &BTreeSet<String>) -> Result<()> {
    for entry in read_dir(schema_path)? {
        let entry = entry.map_err(|source| ProjectError::ReadDirEntry {
            path: schema_path.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str());
        if extension != Some("yaml") && extension != Some("toml") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !expected_tables.contains(file_name) {
            remove_file(&path)?;
        }
    }
    Ok(())
}
