//! Discovery of the project's `.dsql` documents.

use std::fs::{read_dir, read_to_string};
use std::path::{Path, PathBuf};

use dsql_core::source::ResolutionScope;

use super::config::{Project, ProjectError, Result};

/// One discovered document, with its text already read and the resolution
/// scope that owns it.
#[derive(Clone, Debug)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
    pub scope: String,
}

/// Reads every `.dsql` document with its owning scope. Configured paths —
/// plain files, directories, or globs like `queries/shared/**/*.dsql` —
/// resolve from the project *base* (the parent of `dsql/`), matching how
/// real projects lay documents out beside the `dsql/` directory. Without
/// any configuration, everything under `dsql/` itself belongs to the
/// implicit default scope. A document matched by two scopes is a
/// deterministic ownership error.
pub fn load_project_documents(project: &Project) -> Result<Vec<ProjectDocument>> {
    let base = project
        .root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.root.clone());
    let mut owned: Vec<(PathBuf, String)> = Vec::new();

    if project.config.resolution.is_empty() {
        let mut files = Vec::new();
        if project.config.documents.is_empty() {
            collect_dsql_files(&project.root, &mut files)?;
        } else {
            for configured in &project.config.documents {
                collect_configured_path(&base.join(configured), &mut files)?;
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
                collect_configured_path(&base.join(configured), &mut files)?;
            }
            files.sort();
            files.dedup();
            for path in files {
                if let Some((_, first)) = owned.iter().find(|(owned_path, _)| *owned_path == path)
                {
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
            Ok(ProjectDocument { path, text, scope })
        })
        .collect()
}

/// One configured document path: a glob pattern, a directory to walk, or
/// a plain file.
fn collect_configured_path(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
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
                && matched.extension().and_then(|ext| ext.to_str()) == Some("dsql")
            {
                files.push(matched);
            }
        }
        return Ok(());
    }
    collect_dsql_files(path, files)
}

fn collect_dsql_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_file() {
        if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
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
        collect_dsql_files(&entry_path, files)?;
    }
    Ok(())
}
