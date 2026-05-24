use std::path::{Path, PathBuf};

use dsql_embedding::{EmbeddedRegion, RegexEmbedding, default_typescript_regex_pattern};
use miette::Result;

use crate::pipeline::{
    GenerateDocument, GenerateInput, GenerateOptions, GenerateOutput, GeneratedArtifacts,
    ValidationOutput, generate_project, generate_project_artifacts, project_root, validate_project,
};

pub async fn generate_project_from(start_dir: &Path) -> Result<GenerateOutput> {
    generate_project_from_with_options(start_dir, GenerateOptions::default()).await
}

pub async fn generate_project_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<GenerateOutput> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let catalog = project.load_catalog()?;
    let documents = load_project_documents(&project)?;
    let writer = crate::fs::FsArtifactWriter::new(project.root.join("build"));
    let runner = crate::process::CommandGeneratorRunner;
    generate_project(
        GenerateInput {
            project,
            catalog,
            documents,
            options,
        },
        &writer,
        &runner,
    )
    .await
}

pub fn generate_project_artifacts_from(start_dir: &Path) -> Result<GeneratedArtifacts> {
    generate_project_artifacts_from_with_options(start_dir, GenerateOptions::default())
}

pub fn generate_project_artifacts_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<GeneratedArtifacts> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let catalog = project.load_catalog()?;
    let documents = load_project_documents(&project)?;
    generate_project_artifacts(GenerateInput {
        project,
        catalog,
        documents,
        options,
    })
}

pub fn validate_project_from(start_dir: &Path) -> Result<ValidationOutput> {
    validate_project_from_with_options(start_dir, GenerateOptions::default())
}

pub fn validate_project_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<ValidationOutput> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let catalog = project.load_catalog()?;
    let documents = load_project_documents(&project)?;
    Ok(validate_project(GenerateInput {
        project,
        catalog,
        documents,
        options,
    }))
}

fn load_project_documents(project: &dsql_project::Project) -> Result<Vec<GenerateDocument>> {
    let base = project_root(project);
    let mut documents = Vec::new();
    if project.config.documents.is_empty() {
        let mut files = Vec::new();
        collect_dsql_files(&base, Some(&project.root), &mut files)?;
        files.sort();
        files.dedup();
        for path in files {
            documents.push(read_dsql_document(path)?);
        }
    } else {
        for document_config in &project.config.documents {
            let mut files = Vec::new();
            for path in &document_config.paths {
                collect_resolver_path(&base.join(path), Some(&project.root), &mut files)?;
            }
            files.sort();
            files.dedup();

            if document_config.resolver == "dsql" {
                for path in files {
                    if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
                        documents.push(read_dsql_document(path)?);
                    }
                }
            } else {
                let embedding = embedding_for_resolver(project, &document_config.resolver)?;
                for path in files {
                    documents.extend(read_embedded_documents(path, &embedding)?);
                }
            }
        }
    }
    documents.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.source_offset.cmp(&right.source_offset))
    });
    Ok(documents)
}

fn read_dsql_document(path: PathBuf) -> Result<GenerateDocument> {
    let text = std::fs::read_to_string(&path)
        .map_err(|error| miette::miette!("failed to read {}: {error}", path.display()))?;
    Ok(GenerateDocument {
        path,
        text,
        source_offset: 0,
    })
}

fn read_embedded_documents(
    path: PathBuf,
    embedding: &RegexEmbedding,
) -> Result<Vec<GenerateDocument>> {
    let source = std::fs::read_to_string(&path)
        .map_err(|error| miette::miette!("failed to read {}: {error}", path.display()))?;
    embedding
        .extract(&source)?
        .into_iter()
        .map(|region| embedded_document(&path, region))
        .collect()
}

fn embedded_document(path: &Path, region: EmbeddedRegion) -> Result<GenerateDocument> {
    Ok(GenerateDocument {
        path: path.to_path_buf(),
        text: region.text,
        source_offset: region.content_range.start,
    })
}

fn embedding_for_resolver(
    project: &dsql_project::Project,
    resolver: &str,
) -> Result<RegexEmbedding> {
    let pattern = if let Some(config) = project.config.embedding.get(resolver) {
        match config.strategy {
            dsql_project::EmbeddingStrategy::Regex => config.pattern.clone().ok_or_else(|| {
                miette::miette!("embedding `{resolver}` with strategy `regex` requires `pattern`")
            })?,
        }
    } else if resolver == "typescript" {
        default_typescript_regex_pattern()
    } else {
        return Err(miette::miette!(
            "document resolver `{resolver}` requires an [embedding.{resolver}] config"
        ));
    };
    Ok(RegexEmbedding::new(pattern))
}

fn collect_resolver_path(
    path: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if path_has_glob(path) {
        return collect_glob_path(path, excluded_dir, files);
    }
    if path.is_dir() {
        collect_all_files(path, excluded_dir, files)
    } else if path.is_file() {
        files.push(path.to_path_buf());
        Ok(())
    } else {
        Err(miette::miette!(
            "document path not found: {}",
            path.display()
        ))
    }
}

fn collect_glob_path(
    path: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let pattern = path.to_string_lossy();
    for entry in glob::glob(&pattern)
        .map_err(|error| miette::miette!("invalid document glob `{pattern}`: {error}"))?
    {
        let path = entry
            .map_err(|error| miette::miette!("failed to read document glob entry: {error}"))?;
        if excluded_dir.is_some_and(|excluded| path.starts_with(excluded)) {
            continue;
        }
        if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn path_has_glob(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|char| matches!(char, '*' | '?' | '[' | ']'))
}

fn collect_all_files(
    dir: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if excluded_dir.is_some_and(|excluded| dir == excluded) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|error| miette::miette!("failed to read directory {}: {error}", dir.display()))?
    {
        let entry =
            entry.map_err(|error| miette::miette!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_all_files(&path, excluded_dir, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_dsql_files(
    dir: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if excluded_dir.is_some_and(|excluded| dir == excluded) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|error| miette::miette!("failed to read directory {}: {error}", dir.display()))?
    {
        let entry =
            entry.map_err(|error| miette::miette!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_dsql_files(&path, excluded_dir, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path);
        }
    }
    Ok(())
}
