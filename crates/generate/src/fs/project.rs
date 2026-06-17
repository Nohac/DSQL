use std::path::Path;

use crate::Result;
use crate::pipeline::{
    GenerateInput, GenerateOptions, GenerateOutput, GeneratedArtifacts, ValidationOutput,
    generate_project, generate_project_artifacts, validate_project,
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
    let analysis = dsql_frontend::ProjectHost::load_from_project(&project)?;
    let writer = crate::fs::FsArtifactWriter::new(project.root.join("build"));
    let runner = crate::process::CommandGeneratorRunner;
    generate_project(
        GenerateInput {
            project,
            catalog,
            analysis,
            options,
        },
        &writer,
        &runner,
    )
    .await
}

pub async fn generate_project_artifacts_from(start_dir: &Path) -> Result<GeneratedArtifacts> {
    generate_project_artifacts_from_with_options(start_dir, GenerateOptions::default()).await
}

pub async fn generate_project_artifacts_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<GeneratedArtifacts> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let catalog = project.load_catalog()?;
    let analysis = dsql_frontend::ProjectHost::load_from_project(&project)?;
    generate_project_artifacts(GenerateInput {
        project,
        catalog,
        analysis,
        options,
    })
    .await
}

pub async fn validate_project_from(start_dir: &Path) -> Result<ValidationOutput> {
    validate_project_from_with_options(start_dir, GenerateOptions::default()).await
}

pub async fn validate_project_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<ValidationOutput> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let catalog = project.load_catalog()?;
    let analysis = dsql_frontend::ProjectHost::load_from_project(&project)?;
    Ok(validate_project(GenerateInput {
        project,
        catalog,
        analysis,
        options,
    })
    .await)
}
