//! `dsql.toml` discovery and parsing.
//!
//! Deliberately lean: lint, generate, embedding, and resolution-scope
//! configuration return with the phases that consume them.

use std::env::current_dir;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

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
    #[error("failed to build catalog: {0}")]
    CatalogBuild(CatalogBuildError),
}

pub type Result<T> = std::result::Result<T, ProjectError>;

/// The parsed `dsql.toml`.
#[derive(Clone, Debug, Facet)]
pub struct Config {
    pub database_url: String,
    #[facet(default = default_schema())]
    pub default_schema: String,
    /// Paths (relative to the project root) holding `.dsql` documents.
    /// Empty means the project root itself.
    #[facet(default)]
    pub documents: Vec<String>,
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
