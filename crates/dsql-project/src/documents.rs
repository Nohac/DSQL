//! Discovery of the project's document files: plain `.dsql` files and
//! TypeScript host sources. Embedded regions are *not* extracted here —
//! that is a dsql-core system over host text — so the disk loader and the
//! LSP feed the bowl through the same mechanism.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use tokio::fs::{read_dir, read_to_string};

use dsql_core::source::ResolutionScope;

use super::config::{Project, ProjectError, Result};

/// File extensions that carry dsql documents: `.dsql` wholesale, the rest
/// as host sources with embedded regions.
const DOCUMENT_EXTENSIONS: &[&str] = &["dsql", "ts", "tsx"];

/// One discovered document file, with its text already read and the
/// resolution scope that owns it. Whether a file is a dsql document or an
/// embedding host is the source model's call, made at insert time.
#[derive(Clone, Debug)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
    pub scope: String,
}

impl ProjectDocument {
    /// Whether this file is an embedding host (TypeScript) rather than a
    /// plain dsql document — the same classification the source model
    /// applies at insert time.
    pub fn is_embedding_host(&self) -> bool {
        dsql_core::source::is_host_path(&self.path.display().to_string())
    }
}

/// Reads every document with its owning scope. Configured paths — plain
/// files, directories, or globs like `queries/shared/**/*.dsql` — resolve
/// from the project *base* (the parent of `dsql/`), matching how real
/// projects lay documents out beside the `dsql/` directory. Without any
/// configuration, every `.dsql` file under `dsql/` itself belongs to the
/// implicit default scope. A file matched by two scopes is a deterministic
/// ownership error.
pub async fn load_project_documents(project: &Project) -> Result<Vec<ProjectDocument>> {
    let base = project
        .root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.root.clone());
    let build_dir = project.root.join("build");
    let mut owned: Vec<(PathBuf, String)> = Vec::new();

    if project.config.resolution.is_empty() {
        let mut files = Vec::new();
        if project.config.documents.is_empty() {
            collect_dir(&project.root, &["dsql"], &build_dir, &mut files).await?;
        } else {
            for configured in &project.config.documents {
                collect_configured_path(&base.join(configured), &build_dir, &mut files).await?;
            }
        }
        files.sort();
        files.dedup();
        owned.extend(
            files
                .into_iter()
                .map(|path| (path, ResolutionScope::DEFAULT.to_string())),
        );
    } else {
        for (scope, scope_config) in &project.config.resolution {
            let mut files = Vec::new();
            for configured in &scope_config.documents {
                collect_configured_path(&base.join(configured), &build_dir, &mut files).await?;
            }
            files.sort();
            files.dedup();
            for path in files {
                if let Some((_, first)) = owned.iter().find(|(owned_path, _)| *owned_path == path) {
                    return Err(ProjectError::DuplicateScopeDocument {
                        path,
                        first: first.clone(),
                        second: scope.clone(),
                    });
                }
                owned.push((path, scope.clone()));
            }
        }
        owned.sort();
    }

    let mut documents = Vec::new();
    for (path, scope) in owned {
        let text = read_to_string(&path)
            .await
            .map_err(|source| ProjectError::Read {
                path: path.clone(),
                source,
            })?;
        documents.push(ProjectDocument { path, text, scope });
    }
    Ok(documents)
}

/// One configured document path: a glob pattern, a directory to walk, or
/// a plain file. The project's `build/` output never counts as input.
async fn collect_configured_path(
    path: &Path,
    build_dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let raw = path.to_string_lossy();
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        // The glob crate walks the filesystem synchronously; run the whole
        // expansion on the blocking pool.
        let pattern = raw.to_string();
        let pattern_path = path.to_path_buf();
        let build_dir = build_dir.to_path_buf();
        let matched = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
            let matches = glob::glob(&pattern).map_err(|error| ProjectError::Parse {
                path: pattern_path.clone(),
                message: format!("invalid document pattern: {error}"),
            })?;
            let mut collected = Vec::new();
            for entry in matches {
                let matched = entry.map_err(|error| ProjectError::Read {
                    path: pattern_path.clone(),
                    source: error.into_error(),
                })?;
                if matched.is_file()
                    && has_document_extension(&matched, DOCUMENT_EXTENSIONS)
                    && !matched.starts_with(&build_dir)
                {
                    collected.push(matched);
                }
            }
            Ok(collected)
        })
        .await
        .map_err(|error| ProjectError::Parse {
            path: path.to_path_buf(),
            message: format!("document pattern task failed: {error}"),
        })??;
        files.extend(matched);
        return Ok(());
    }
    let metadata = tokio::fs::metadata(path).await.ok();
    if metadata.as_ref().is_some_and(std::fs::Metadata::is_file) {
        if has_document_extension(path, DOCUMENT_EXTENSIONS) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    collect_dir(path, DOCUMENT_EXTENSIONS, build_dir, files).await
}

fn has_document_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| extensions.contains(&ext))
}

/// Boxed for async recursion.
fn collect_dir<'a>(
    path: &'a Path,
    extensions: &'a [&'a str],
    build_dir: &'a Path,
    files: &'a mut Vec<PathBuf>,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let metadata = tokio::fs::metadata(path).await.ok();
        if !metadata.as_ref().is_some_and(std::fs::Metadata::is_dir) || path == build_dir {
            return Ok(());
        }
        let mut entries = read_dir(path).await.map_err(|source| ProjectError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        loop {
            let entry = entries
                .next_entry()
                .await
                .map_err(|source| ProjectError::Read {
                    path: path.to_path_buf(),
                    source,
                })?;
            let Some(entry) = entry else {
                return Ok(());
            };
            let entry_path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .map_err(|source| ProjectError::Read {
                    path: entry_path.clone(),
                    source,
                })?;
            if file_type.is_dir() {
                collect_dir(&entry_path, extensions, build_dir, files).await?;
            } else if has_document_extension(&entry_path, extensions) {
                files.push(entry_path);
            }
        }
    })
}
