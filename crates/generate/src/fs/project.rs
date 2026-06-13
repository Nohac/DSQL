use std::path::Path;

use crate::Result;
use crate::pipeline::{
    GenerateDocument, GenerateInput, GenerateOptions, GenerateOutput, GeneratedArtifacts,
    ValidationOutput, generate_project, generate_project_artifacts, validate_project,
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
    let documents = load_generate_documents(&project)?;
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
    let documents = load_generate_documents(&project)?;
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
    let documents = load_generate_documents(&project)?;
    Ok(validate_project(GenerateInput {
        project,
        catalog,
        documents,
        options,
    }))
}

fn load_generate_documents(project: &dsql_project::Project) -> Result<Vec<GenerateDocument>> {
    Ok(dsql_project::load_project_documents(project)?
        .into_iter()
        .map(|document| GenerateDocument {
            path: document.path,
            text: document.text,
            source_offset: document.source_offset as u32,
            resolution_scope: document.resolution_scope,
        })
        .collect())
}
