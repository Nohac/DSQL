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
    #[error(
        "cannot generate while diagnostics contain errors{}",
        language_diagnostics_details(diagnostics, errors)
    )]
    LanguageDiagnostics {
        diagnostics: Vec<pipeline::ValidationDiagnostic>,
        errors: Vec<pipeline::ValidationError>,
    },
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

impl From<pipeline::ValidationError> for GenerateError {
    fn from(error: pipeline::ValidationError) -> Self {
        Self::LanguageDiagnostics {
            diagnostics: Vec::new(),
            errors: vec![error],
        }
    }
}

fn language_diagnostics_details(
    diagnostics: &[pipeline::ValidationDiagnostic],
    errors: &[pipeline::ValidationError],
) -> String {
    let mut details = String::new();
    for diagnostic in diagnostics {
        let start = diagnostic.source_offset + diagnostic.diagnostic.range.start;
        let end = diagnostic.source_offset + diagnostic.diagnostic.range.end;
        details.push_str(&format!(
            "\n{} {:?} {:?} {}..{}: {}",
            diagnostic.file.display(),
            diagnostic.diagnostic.source,
            diagnostic.diagnostic.code,
            start,
            end,
            diagnostic.diagnostic.message
        ));
    }
    for error in errors {
        if let (Some(file), Some(range), Some(source_offset)) =
            (&error.file, error.range, error.source_offset)
        {
            details.push_str(&format!(
                "\n{} Generate {:?} {}..{}: {}",
                file.display(),
                error.kind,
                source_offset + range.start,
                source_offset + range.end,
                error.message
            ));
        } else {
            details.push_str(&format!("\nGenerate {:?}: {}", error.kind, error.message));
        }
    }
    details
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
    GenerateOptions, GenerateOutput, GeneratedArtifactGroup, GeneratedArtifacts,
    GeneratedFragmentArtifact, GeneratedOperationArtifact, ValidationDiagnostic, ValidationError,
    ValidationErrorKind, ValidationOutput,
};

#[cfg(all(feature = "fs", feature = "process"))]
pub use fs::{
    generate_project_artifacts_from, generate_project_artifacts_from_with_options,
    generate_project_from, generate_project_from_with_options, validate_project_from,
    validate_project_from_with_options,
};
