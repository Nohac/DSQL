mod pipeline;

mod artifacts;
mod layout;
mod runner;

#[cfg(feature = "fs")]
pub mod fs;

#[cfg(feature = "process")]
pub mod process;

pub type Result<T> = std::result::Result<T, GenerateError>;

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("no dsql documents found in project {project}")]
    NoDocuments { project: String },
    #[error("cannot generate while diagnostics contain errors\n{details}")]
    LanguageDiagnostics { details: String },
    #[error("artifact write failed: {0}")]
    Artifact(#[from] ArtifactError),
    #[error("external generator failed: {0}")]
    Generator(#[from] GeneratorError),
    #[error("project error: {0}")]
    Project(#[from] dsql_project::ProjectError),
    #[error("{0}")]
    Other(String),
}

impl miette::Diagnostic for GenerateError {}

impl From<miette::Report> for GenerateError {
    fn from(error: miette::Report) -> Self {
        Self::Other(error.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize artifact JSON: {0}")]
    SerializeJson(String),
    #[error("failed to write {path}: {source}")]
    WriteFile {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("artifact path is not valid UTF-8: {path}")]
    NonUtf8Path { path: std::path::PathBuf },
}

impl miette::Diagnostic for ArtifactError {}

#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    #[error("failed to run generator `{program}`: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("generator `{command}` failed with status {status}")]
    Failed {
        command: String,
        status: std::process::ExitStatus,
    },
}

impl miette::Diagnostic for GeneratorError {}

pub use pipeline::{
    GenerateOptions, GenerateOutput, GeneratedArtifacts, GeneratedFragmentArtifact,
    GeneratedOperationArtifact, ValidationDiagnostic, ValidationOutput,
};

#[cfg(all(feature = "fs", feature = "process"))]
pub use fs::{
    generate_project_artifacts_from, generate_project_artifacts_from_with_options,
    generate_project_from, generate_project_from_with_options, validate_project_from,
    validate_project_from_with_options,
};
