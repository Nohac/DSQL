//! Discovery of the project's documents: plain `.dsql` files and dsql
//! regions embedded in TypeScript sources.

use std::fs::{read_dir, read_to_string};
use std::path::{Path, PathBuf};

use regex::Regex;

use dsql_core::source::ResolutionScope;

use super::config::{Project, ProjectError, Result};
use super::embedding::{extract_regions, typescript_embedding};

/// File extensions that carry dsql documents: `.dsql` wholesale, the rest
/// through embedded-region extraction.
const DOCUMENT_EXTENSIONS: &[&str] = &["dsql", "ts", "tsx"];

/// One discovered document, with its text already read and the resolution
/// scope that owns it.
#[derive(Clone, Debug)]
pub struct ProjectDocument {
    pub path: PathBuf,
    pub text: String,
    /// Byte offset of the text inside its host file: zero for `.dsql`
    /// files, the region start for embedded documents.
    pub source_offset: usize,
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
    let embedding = typescript_embedding(project)?;
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

    let mut documents = Vec::new();
    for (path, scope) in owned {
        read_documents(&path, &scope, &embedding, &mut documents)?;
    }
    Ok(documents)
}

/// Reads one file's documents: a `.dsql` file wholesale, anything else
/// through embedded-region extraction.
fn read_documents(
    path: &Path,
    scope: &str,
    embedding: &Regex,
    documents: &mut Vec<ProjectDocument>,
) -> Result<()> {
    let text = read_to_string(path).map_err(|source| ProjectError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
        documents.push(ProjectDocument {
            path: path.to_path_buf(),
            text,
            source_offset: 0,
            scope: scope.to_string(),
        });
        return Ok(());
    }
    documents.extend(extract_regions(embedding, &text).into_iter().map(|region| {
        ProjectDocument {
            path: path.to_path_buf(),
            text: region.text,
            source_offset: region.offset,
            scope: scope.to_string(),
        }
    }));
    Ok(())
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
