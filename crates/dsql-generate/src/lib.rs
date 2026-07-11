//! Artifact generation: assembles per-operation and per-fragment metadata
//! from a settled project bowl and writes the `build/` tree the host
//! generator consumes.

mod assemble;
mod layout;
mod pipeline;

pub use pipeline::{
    GenerateError, GenerateOptions, GenerateOutput, generate_project, validate_assembly,
};
