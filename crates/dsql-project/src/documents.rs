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
    load_project_documents_excluding(project, &[]).await
}

/// [`load_project_documents`] with additional reserved roots excluded —
/// consumer-owned generated directories a daemon binding declares
/// (docs/spec/build-daemon.md): generated code is never project input,
/// no matter which side generated it. Generator outputs from the project
/// configuration are always excluded.
pub async fn load_project_documents_excluding(
    project: &Project,
    extra_reserved: &[String],
) -> Result<Vec<ProjectDocument>> {
    let base = project
        .root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.root.clone());
    let build_dir = project.root.join("build");
    let reserved: Vec<PathBuf> = std::iter::once(build_dir.clone())
        .chain(
            project
                .config
                .generate
                .typescript
                .outputs
                .iter()
                .chain(extra_reserved)
                .map(|root| base.join(root.trim_matches('/'))),
        )
        .collect();
    let is_reserved = |path: &Path| reserved.iter().any(|root| path.starts_with(root));
    let mut owned: Vec<(PathBuf, String)> = Vec::new();

    if project.config.resolution.is_empty() {
        let mut files = Vec::new();
        if project.config.documents.is_empty() {
            collect_dir(&project.root, &["dsql"], &reserved, &mut files).await?;
        } else {
            for configured in &project.config.documents {
                collect_configured_path(&base.join(configured), &reserved, &mut files).await?;
            }
        }
        files.sort();
        files.dedup();
        files.retain(|path| !is_reserved(path));
        owned.extend(
            files
                .into_iter()
                .map(|path| (path, ResolutionScope::DEFAULT.to_string())),
        );
    } else {
        for (scope, scope_config) in &project.config.resolution {
            let mut files = Vec::new();
            for configured in &scope_config.documents {
                collect_configured_path(&base.join(configured), &reserved, &mut files).await?;
            }
            files.sort();
            files.dedup();
            // Reserved roots drop BEFORE ownership conflicts: two scopes
            // overlapping inside a generated directory is not an error,
            // it is not input at all.
            files.retain(|path| !is_reserved(path));
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
    reserved: &[PathBuf],
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let raw = path.to_string_lossy();
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        // A manual walk with glob *matching*: `glob::glob` would traverse
        // the whole pattern space — including reserved generated trees,
        // whose size or permissions must never affect discovery. The walk
        // starts at the pattern's static prefix and prunes reserved
        // directories at the boundary.
        let pattern = raw.to_string();
        let pattern_path = path.to_path_buf();
        let reserved = reserved.to_vec();
        let matched = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
            let matcher = glob::Pattern::new(&pattern).map_err(|error| ProjectError::Parse {
                path: pattern_path.clone(),
                message: format!("invalid document pattern: {error}"),
            })?;
            let static_prefix: PathBuf = pattern_path
                .components()
                .take_while(|component| {
                    let text = component.as_os_str().to_string_lossy();
                    !text.contains(['*', '?', '['])
                })
                .collect();
            let mut collected = Vec::new();
            let mut stack = vec![static_prefix];
            while let Some(current) = stack.pop() {
                if reserved.iter().any(|root| current.starts_with(root)) {
                    continue;
                }
                let Ok(metadata) = std::fs::metadata(&current) else {
                    continue;
                };
                if metadata.is_file() {
                    // `glob::glob` requires literal separators — `*`
                    // must not cross directory boundaries.
                    let options = glob::MatchOptions {
                        require_literal_separator: true,
                        ..glob::MatchOptions::new()
                    };
                    if matcher.matches_path_with(&current, options)
                        && has_document_extension(&current, DOCUMENT_EXTENSIONS)
                    {
                        collected.push(current);
                    }
                    continue;
                }
                let entries = std::fs::read_dir(&current).map_err(|source| ProjectError::Read {
                    path: current.clone(),
                    source,
                })?;
                for entry in entries {
                    let entry = entry.map_err(|source| ProjectError::Read {
                        path: current.clone(),
                        source,
                    })?;
                    stack.push(entry.path());
                }
            }
            collected.sort();
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
    collect_dir(path, DOCUMENT_EXTENSIONS, reserved, files).await
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
    reserved: &'a [PathBuf],
    files: &'a mut Vec<PathBuf>,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let metadata = tokio::fs::metadata(path).await.ok();
        // Reserved subtrees are pruned at the directory boundary — a
        // large generated tree must never even be scanned.
        if !metadata.as_ref().is_some_and(std::fs::Metadata::is_dir)
            || reserved.iter().any(|root| path.starts_with(root))
        {
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
                collect_dir(&entry_path, extensions, reserved, files).await?;
            } else if has_document_extension(&entry_path, extensions) {
                files.push(entry_path);
            }
        }
    })
}
