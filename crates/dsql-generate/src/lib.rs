//! Artifact generation: assembles per-operation and per-fragment metadata
//! from a settled project bowl and writes the `build/` tree the host
//! generator consumes.

mod assemble;
mod layout;
mod pipeline;
pub mod publish;

pub use pipeline::{
    AssembledProject, GenerateError, GenerateOptions, GenerateOutput, assemble_project,
    generate_project, publish_snapshot, validate_assembly,
};
