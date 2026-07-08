//! Discovery of the project's `.dsql` documents.

use std::fs::{read_dir, read_to_string};
use std::path::{Path, PathBuf};

use super::config::{Project, ProjectError, Result};

/// One discovered document, with its text already read.
#[derive(Clone, Debug)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
}

/// Reads every `.dsql` file under the configured document paths (or the
/// project root when none are configured), sorted by path.
pub fn load_project_documents(project: &Project) -> Result<Vec<ProjectDocument>> {
    let mut files = Vec::new();
    if project.config.documents.is_empty() {
        collect_dsql_files(&project.root, &mut files)?;
    } else {
        for configured in &project.config.documents {
            collect_dsql_files(&project.root.join(configured), &mut files)?;
        }
    }
    files.sort();
    files.dedup();

    files
        .into_iter()
        .map(|path| {
            let text = read_to_string(&path).map_err(|source| ProjectError::Read {
                path: path.clone(),
                source,
            })?;
            Ok(ProjectDocument { path, text })
        })
        .collect()
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
