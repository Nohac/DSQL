//! `dsql.toml` discovery and parsing.
//!
//! Deliberately lean: lint configuration returns with the phase that
//! consumes it.

use std::env::current_dir;
use std::path::{Path, PathBuf};

use tokio::fs::read_to_string;

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
    #[error("a dsql project already exists at {0}")]
    AlreadyInitialized(PathBuf),
    #[error("generator output `{output}` {problem}")]
    InvalidGeneratorOutput { output: String, problem: String },
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
    /// The command's output directories, project-base-relative. Required
    /// for daemon-driven builds (docs/spec/build-daemon.md, Host
    /// generator command): consumers exclude them from watching, and the
    /// daemon skips an enabled command that declares none.
    #[facet(default)]
    pub outputs: Vec<String>,
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
    pub async fn load() -> Result<Self> {
        let start = current_dir().map_err(ProjectError::CurrentDir)?;
        Self::load_from(&start).await
    }

    pub async fn load_from(start_dir: &Path) -> Result<Self> {
        let root = find_root(start_dir)
            .await
            .ok_or_else(|| ProjectError::MissingRoot(start_dir.to_path_buf()))?;
        let config_path = root.join("dsql.toml");
        let raw = read_to_string(&config_path)
            .await
            .map_err(|source| ProjectError::Read {
                path: config_path.clone(),
                source,
            })?;
        let config: Config = facet_toml::from_str(&raw).map_err(|error| ProjectError::Parse {
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
        let mut config = config;
        let normalized_outputs = config
            .generate
            .typescript
            .outputs
            .iter()
            .map(|output| validate_reserved_root(&config, output))
            .collect::<Result<Vec<_>>>()?;
        config.generate.typescript.outputs = normalized_outputs;
        Ok(Self {
            schema: root.join("schema"),
            root,
            config,
        })
    }

    /// Loads the schema catalog from the project's schema directory.
    pub async fn load_catalog(&self) -> Result<Catalog> {
        let metadata = super::metadata::load_metadata_dir(&self.schema).await?;
        metadata
            .into_catalog()
            .map(|catalog| catalog.with_default_schema(self.config.default_schema.clone()))
            .map_err(ProjectError::CatalogBuild)
    }
}

/// Validates one reserved root (a generator output or a consumer's
/// `excludeRoots` entry) against the config, returning its normalized
/// form. Absoluteness and traversal are judged on the RAW value —
/// trimming first would launder `/tmp/out` into `tmp/out`.
pub fn validate_reserved_root(config: &Config, output: &str) -> Result<String> {
    let reject = |problem: &str| {
        Err(ProjectError::InvalidGeneratorOutput {
            output: output.to_string(),
            problem: problem.to_string(),
        })
    };
    if Path::new(output).is_absolute() {
        return reject("must be a project-base-relative directory");
    }
    for component in Path::new(output).components() {
        use std::path::Component;
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return reject("must not traverse out of the project"),
            Component::RootDir | Component::Prefix(_) => {
                return reject("must be a project-base-relative directory");
            }
        }
    }
    // Rebuild from components so `./generated` and `generated/` both
    // normalize to `generated`.
    let normalized = Path::new(output)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        return reject("must not be the project base itself");
    }
    let contains =
        |outer: &str, inner: &str| inner == outer || inner.starts_with(&format!("{outer}/"));
    for reserved in ["dsql/dsql.toml", "dsql"] {
        if contains(&normalized, reserved) || contains(reserved, &normalized) {
            return reject("must be disjoint from the project's own dsql/ tree");
        }
    }
    let document_patterns = config
        .resolution
        .values()
        .flat_map(|scope| scope.documents.iter())
        .chain(config.documents.iter());
    for pattern in document_patterns {
        let prefix: String = pattern
            .split('/')
            .take_while(|segment| !segment.contains(['*', '?', '[']))
            .collect::<Vec<_>>()
            .join("/");
        if !prefix.is_empty()
            && (prefix == normalized || prefix.starts_with(&format!("{normalized}/")))
        {
            return reject("must not contain a configured document root");
        }
    }
    Ok(normalized)
}

/// The starter file's serialized header: routing the URL through
/// facet_toml escapes whatever characters it carries.
#[derive(Facet)]
struct StarterHeader {
    database_url: String,
    default_schema: String,
}

/// Scaffolds a new project under `base_path`: a `dsql/` root with a
/// starter `dsql.toml` (one `main` scope owning every `.dsql` document)
/// and an empty `schema/` directory. Refuses to touch an existing
/// project.
pub async fn init_project(base_path: &Path, database_url: Option<String>) -> Result<Project> {
    let root = base_path.join("dsql");
    let config_path = root.join("dsql.toml");

    // The complete starter is composed and parsed back through the real
    // Config BEFORE anything touches disk: a URL facet_toml cannot
    // represent, or template drift, fails cleanly instead of stranding a
    // half-initialized project.
    let header = StarterHeader {
        database_url: database_url.unwrap_or_else(|| "<database url>".to_string()),
        default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
    };
    let header = facet_toml::to_string(&header).map_err(|error| ProjectError::Parse {
        path: config_path.clone(),
        message: error.to_string(),
    })?;
    let raw = format!(
        "{header}\n\
         # Every .dsql document under the project base belongs to `main`.\n\
         # Add more scopes (and `imports`) to partition definitions.\n\
         [resolution.main]\ndocuments = [\"**/*.dsql\"]\n"
    );
    let config: Config = facet_toml::from_str(&raw).map_err(|error| ProjectError::Parse {
        path: config_path.clone(),
        message: error.to_string(),
    })?;

    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|source| ProjectError::Write {
            path: root.clone(),
            source,
        })?;
    // create_new is the existence check: no separate probe to race with.
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = match options.open(&config_path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ProjectError::AlreadyInitialized(config_path));
        }
        Err(source) => {
            return Err(ProjectError::Write {
                path: config_path,
                source,
            });
        }
    };
    // Any failure past this point rolls the config back: a stranded
    // dsql.toml would turn every retry into AlreadyInitialized. Each step
    // reports its own path.
    let schema = root.join("schema");
    let committed = async {
        tokio::io::AsyncWriteExt::write_all(&mut file, raw.as_bytes())
            .await
            .map_err(|source| ProjectError::Write {
                path: config_path.clone(),
                source,
            })?;
        tokio::io::AsyncWriteExt::flush(&mut file)
            .await
            .map_err(|source| ProjectError::Write {
                path: config_path.clone(),
                source,
            })?;
        drop(file);
        tokio::fs::create_dir_all(&schema)
            .await
            .map_err(|source| ProjectError::Write {
                path: schema.clone(),
                source,
            })
    }
    .await;
    if let Err(error) = committed {
        let _ = tokio::fs::remove_file(&config_path).await;
        return Err(error);
    }
    Ok(Project {
        root,
        schema,
        config,
    })
}

/// Walks up from `start_dir` looking for a `dsql/dsql.toml` root.
pub async fn find_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current_dir = start_dir;
    loop {
        let candidate = current_dir.join("dsql");
        if tokio::fs::try_exists(candidate.join("dsql.toml"))
            .await
            .unwrap_or(false)
        {
            return Some(candidate);
        }
        current_dir = current_dir.parent()?;
    }
}
