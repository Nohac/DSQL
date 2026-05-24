mod pipeline;

mod artifacts;
mod runner;

#[cfg(feature = "fs")]
pub mod fs;

#[cfg(feature = "process")]
pub mod process;

pub use pipeline::{
    GenerateOptions, GenerateOutput, GeneratedArtifacts, GeneratedOperationArtifact,
};

#[cfg(all(feature = "fs", feature = "process"))]
pub use fs::{
    generate_project_artifacts_from, generate_project_artifacts_from_with_options,
    generate_project_from, generate_project_from_with_options,
};
