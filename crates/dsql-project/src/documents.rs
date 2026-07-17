//! Discovery of physical project sources selected by resolver-bearing
//! `dsql.toml` document groups. Embedded regions are not extracted here — the
//! configured resolver travels with the host into the bowl.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use tokio::fs::{read_dir, read_to_string};

use dsql_core::source::{ResolutionScope, ScopeDocument, SourceKind};

use super::config::{DocumentConfig, Project, ProjectError, Result};

/// One discovered physical source, with its text, owning scope, and configured
/// resolver classification already attached.
#[derive(Clone, Debug)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
    pub scope: String,
    pub kind: SourceKind,
}

/// Reads every document with its owning scope. Configured paths — plain
/// files, directories, or globs like `queries/shared/**/*.dsql` — resolve
/// from the project *base* (the parent of `dsql/`), matching how real
/// projects lay documents out beside the `dsql/` directory. Without any
/// configuration, every `.dsql` file under `dsql/` itself belongs to the
/// implicit default scope. More than one `(scope, resolver)` assignment for a
/// file is a deterministic ownership error.
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
    let base = project.base();
    let reserved: Vec<PathBuf> = std::iter::once(project.root.join("build"))
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
    let mut owned: Vec<(PathBuf, String, SourceKind)> = Vec::new();

    if project.config.uses_implicit_documents() {
        // Preserve the legacy cold-path directory walk: unlike configured
        // glob discovery it neither follows symlinked directories nor parses
        // metacharacters in the project-root path.
        let mut files = Vec::new();
        collect_dir(&project.root, true, &reserved, &mut files).await?;
        files.sort();
        files.dedup();
        for path in files {
            owned.push((path, ResolutionScope::DEFAULT.to_string(), SourceKind::Dsql));
        }
    } else {
        for (scope, documents) in project.config.document_scopes() {
            for document in documents {
                collect_document_group(base, scope, document, &reserved, &mut owned).await?;
            }
        }
    }
    owned.sort();

    let mut documents = Vec::new();
    for (path, scope, kind) in owned {
        let text = read_to_string(&path)
            .await
            .map_err(|source| ProjectError::Read {
                path: path.clone(),
                source,
            })?;
        documents.push(ProjectDocument {
            path,
            text,
            scope,
            kind,
        });
    }
    Ok(documents)
}

async fn collect_document_group(
    base: &Path,
    scope: &str,
    document: &DocumentConfig,
    reserved: &[PathBuf],
    owned: &mut Vec<(PathBuf, String, SourceKind)>,
) -> Result<()> {
    let kind = SourceKind::from_resolver(document.resolver.clone());
    let mut files = Vec::new();
    for configured in &document.paths {
        collect_configured_path(&base.join(configured), reserved, &mut files).await?;
    }
    files.sort();
    files.dedup();
    files.retain(|path| !reserved.iter().any(|root| path.starts_with(root)));
    for path in files {
        if let Some((_, first_scope, first_kind)) =
            owned.iter().find(|(owned_path, _, _)| *owned_path == path)
        {
            return Err(ProjectError::DuplicateDocumentAssignment {
                path,
                first_scope: first_scope.clone(),
                first_resolver: first_kind.resolver().to_string(),
                second_scope: scope.to_string(),
                second_resolver: kind.resolver().to_string(),
            });
        }
        owned.push((path, scope.to_string(), kind.clone()));
    }
    Ok(())
}

/// Resolver-bearing absolute path assignments installed in the bowl for live
/// LSP and daemon ownership decisions.
pub(crate) fn scope_document_assignments(project: &Project) -> Vec<(String, Vec<ScopeDocument>)> {
    let base = project.base();
    if project.config.uses_implicit_documents() {
        return vec![(
            ResolutionScope::DEFAULT.to_string(),
            vec![ScopeDocument {
                kind: SourceKind::Dsql,
                paths: vec![project.root.join("**/*.dsql").display().to_string()],
            }],
        )];
    }
    project
        .config
        .document_scopes()
        .map(|(scope, documents)| {
            (
                scope.to_string(),
                documents
                    .iter()
                    .map(|document| ScopeDocument {
                        kind: SourceKind::from_resolver(document.resolver.clone()),
                        paths: document
                            .paths
                            .iter()
                            .map(|path| base.join(path).display().to_string())
                            .collect(),
                    })
                    .collect(),
            )
        })
        .collect()
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
                    if matcher.matches_path_with(&current, options) {
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
        files.push(path.to_path_buf());
        return Ok(());
    }
    collect_dir(path, false, reserved, files).await
}

/// Boxed for async recursion.
fn collect_dir<'a>(
    path: &'a Path,
    dsql_only: bool,
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
                collect_dir(&entry_path, dsql_only, reserved, files).await?;
            } else if !dsql_only
                || entry_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("dsql")
            {
                files.push(entry_path);
            }
        }
    })
}
