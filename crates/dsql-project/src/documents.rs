//! Discovery of the project's document files: plain `.dsql` files and
//! TypeScript host sources. Embedded regions are *not* extracted here —
//! that is a dsql-core system over host text — so the disk loader and the
//! LSP feed the bowl through the same mechanism.

use std::fs::{read_dir, read_to_string};
use std::path::{Path, PathBuf};

use dsql_core::source::ResolutionScope;

use super::config::{Project, ProjectError, Result};

/// File extensions that carry dsql documents: `.dsql` wholesale, the rest
/// as host sources with embedded regions.
const DOCUMENT_EXTENSIONS: &[&str] = &["dsql", "ts", "tsx"];

/// What a document file contains: dsql text, or another language's text
/// with dsql regions embedded in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Dsql,
    EmbeddingHost,
}

/// One discovered document file, with its text already read and the
/// resolution scope that owns it.
#[derive(Clone, Debug)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
    pub kind: DocumentKind,
    pub scope: String,
}

/// Reads every document with its owning scope. Configured paths — plain
/// files, directories, or globs like `queries/shared/**/*.dsql` — resolve
/// from the project *base* (the parent of `dsql/`), matching how real
/// projects lay documents out beside the `dsql/` directory. Without any
/// configuration, every `.dsql` file under `dsql/` itself belongs to the
/// implicit default scope. A file matched by two scopes is a deterministic
/// ownership error.
pub fn load_project_documents(project: &Project) -> Result<Vec<ProjectDocument>> {
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
            collect_dir(&project.root, &["dsql"], &build_dir, &mut files)?;
        } else {
            for configured in &project.config.documents {
                collect_configured_path(&base.join(configured), &build_dir, &mut files)?;
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
                collect_configured_path(&base.join(configured), &build_dir, &mut files)?;
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

    owned
        .into_iter()
        .map(|(path, scope)| {
            let text = read_to_string(&path).map_err(|source| ProjectError::Read {
                path: path.clone(),
                source,
            })?;
            let kind = if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
                DocumentKind::Dsql
            } else {
                DocumentKind::EmbeddingHost
            };
            Ok(ProjectDocument {
                path,
                text,
                kind,
                scope,
            })
        })
        .collect()
}

/// One configured document path: a glob pattern, a directory to walk, or
/// a plain file. The project's `build/` output never counts as input.
fn collect_configured_path(path: &Path, build_dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let raw = path.to_string_lossy();
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        let pattern = raw.to_string();
        let matches = glob::glob(&pattern).map_err(|error| ProjectError::Parse {
            path: path.to_path_buf(),
            message: format!("invalid document pattern: {error}"),
        })?;
        for entry in matches {
            let matched = entry.map_err(|error| ProjectError::Read {
                path: path.to_path_buf(),
                source: error.into_error(),
            })?;
            if matched.is_file()
                && has_document_extension(&matched, DOCUMENT_EXTENSIONS)
                && !matched.starts_with(build_dir)
            {
                files.push(matched);
            }
        }
        return Ok(());
    }
    if path.is_file() {
        if has_document_extension(path, DOCUMENT_EXTENSIONS) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    collect_dir(path, DOCUMENT_EXTENSIONS, build_dir, files)
}

fn has_document_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| extensions.contains(&ext))
}

fn collect_dir(
    path: &Path,
    extensions: &[&str],
    build_dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if !path.is_dir() || path == build_dir {
        return Ok(());
    }
    for entry in read_dir(path).map_err(|source| ProjectError::Read {
        path: path.to_path_buf(),
        source,
    })? {
        let entry_path = entry
            .map_err(|source| ProjectError::Read {
                path: path.to_path_buf(),
                source,
            })?
            .path();
        if entry_path.is_dir() {
            collect_dir(&entry_path, extensions, build_dir, files)?;
        } else if has_document_extension(&entry_path, extensions) {
            files.push(entry_path);
        }
    }
    Ok(())
}
