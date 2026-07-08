//! `dsql.toml` discovery and parsing.
//!
//! Deliberately lean: lint configuration returns with the phase that
//! consumes it.

use std::env::current_dir;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use facet::Facet;

use dsql_core::catalog::{Catalog, CatalogBuildError};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("no dsql project found (missing dsql/dsql.toml above {0})")]
    MissingRoot(PathBuf),
    #[error("failed to resolve current directory: {0}")]
    CurrentDir(std::io::Error),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to build catalog: {0}")]
    CatalogBuild(CatalogBuildError),
    #[error("document {path} is owned by both scope `{first}` and scope `{second}`")]
    DuplicateScopeDocument {
        path: PathBuf,
        first: String,
        second: String,
    },
    #[error("invalid embedding pattern for `{language}`: {message}")]
    InvalidEmbeddingPattern { language: String, message: String },
    #[error("scope `{scope}` imports unknown scope `{import}`")]
    UnknownScopeImport { scope: String, import: String },
}

pub type Result<T> = std::result::Result<T, ProjectError>;

/// The parsed `dsql.toml`.
#[derive(Clone, Debug, Facet)]
pub struct Config {
    pub database_url: String,
    #[facet(default = default_schema())]
    pub default_schema: String,
    /// Paths (relative to the project root) holding `.dsql` documents.
    /// Empty means the project root itself. Only consulted when no
    /// resolution scopes are configured — every document then belongs to
    /// the implicit `default` scope.
    #[facet(default)]
    pub documents: Vec<String>,
    /// Named resolution scopes (docs/spec/resolution-scopes.md). Each
    /// document belongs to exactly one scope; imports make another scope's
    /// definitions visible.
    #[facet(default)]
    pub resolution: BTreeMap<String, ScopeConfig>,
    /// Artifact generation configuration.
    #[facet(default)]
    pub generate: GenerateConfig,
    /// Embedded-document extraction, per host language.
    #[facet(default)]
    pub embedding: EmbeddingConfig,
    /// Lint severities.
    #[facet(default)]
    pub lint: LintSectionConfig,
}

/// The `[lint]` section.
#[derive(Clone, Debug, Default, Facet)]
pub struct LintSectionConfig {
    /// Severity of the unindexed-scan lint family; unset means `info`,
    /// `off` disables it.
    #[facet(default)]
    pub unindexed_scan_severity: Option<LintSeverity>,
}

#[derive(Clone, Copy, Debug, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(C)]
pub enum LintSeverity {
    Off,
    Info,
    Warning,
    Error,
}

/// The `[embedding.*]` sections.
#[derive(Clone, Debug, Default, Facet)]
pub struct EmbeddingConfig {
    #[facet(default)]
    pub typescript: TypescriptEmbeddingConfig,
}

/// `[embedding.typescript]`: how dsql documents are found inside `.ts` and
/// `.tsx` sources. The pattern is a regex with a named `content` capture;
/// unset means the default `dsql`-tagged-template pattern.
#[derive(Clone, Debug, Default, Facet)]
pub struct TypescriptEmbeddingConfig {
    #[facet(default)]
    pub pattern: Option<String>,
}

/// The `[generate.*]` sections.
#[derive(Clone, Debug, Default, Facet)]
pub struct GenerateConfig {
    #[facet(default)]
    pub typescript: TypescriptGenerateConfig,
}

/// `[generate.typescript]`: a host command run after the `build/` tree is
/// written, from the project base directory (the parent of `dsql/`).
#[derive(Clone, Debug, Default, Facet)]
pub struct TypescriptGenerateConfig {
    #[facet(default)]
    pub enabled: bool,
    #[facet(default)]
    pub cmd: Vec<String>,
}

/// One `[resolution.<name>]` section.
#[derive(Clone, Debug, Facet)]
pub struct ScopeConfig {
    /// Paths (relative to the project root) whose `.dsql` documents this
    /// scope owns.
    #[facet(default)]
    pub documents: Vec<String>,
    /// Scopes whose definitions this scope imports (not transitive).
    #[facet(default)]
    pub imports: Vec<String>,
}

fn default_schema() -> String {
    Catalog::DEFAULT_SCHEMA.to_string()
}

/// A loaded project: the `dsql/` root, its schema directory, and config.
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
        let root = find_root(start_dir)
            .ok_or_else(|| ProjectError::MissingRoot(start_dir.to_path_buf()))?;
        let config_path = root.join("dsql.toml");
        let raw = read_to_string(&config_path).map_err(|source| ProjectError::Read {
            path: config_path.clone(),
            source,
        })?;
        let config: Config =
            facet_toml::from_str(&raw).map_err(|error| ProjectError::Parse {
                path: config_path,
                message: error.to_string(),
            })?;
        for (scope, scope_config) in &config.resolution {
            for import in &scope_config.imports {
                if !config.resolution.contains_key(import) {
                    return Err(ProjectError::UnknownScopeImport {
                        scope: scope.clone(),
                        import: import.clone(),
                    });
                }
            }
        }
        Ok(Self {
            schema: root.join("schema"),
            root,
            config,
        })
    }

    /// Loads the schema catalog from the project's schema directory.
    pub fn load_catalog(&self) -> Result<Catalog> {
        let metadata = super::metadata::load_metadata_dir(&self.schema)?;
        metadata
            .into_catalog()
            .map(|catalog| catalog.with_default_schema(self.config.default_schema.clone()))
            .map_err(ProjectError::CatalogBuild)
    }
}

/// Walks up from `start_dir` looking for a `dsql/dsql.toml` root.
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
