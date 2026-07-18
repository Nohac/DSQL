//! Artifact generation: assembles per-operation and per-fragment metadata
//! from a settled project bowl and writes the `build/` tree the host
//! generator consumes.

mod assemble;
#[cfg(feature = "native")]
mod layout;
mod match_lock;
#[cfg(feature = "native")]
mod native;
mod pipeline;
#[cfg(feature = "native")]
pub mod publish;
pub mod snapshot;

pub use match_lock::{
    FILTER_MATCH_LOCK_VERSION, FilterMatchLock, LockedFilter, LockedFilterMatch,
    LockedPolicyReference,
};
#[cfg(feature = "native")]
pub use native::{
    GenerateOutput, assemble_project, generate_project, publish_snapshot,
    reconcile_project_match_lock, validate_assembly,
};
pub use pipeline::{
    AssembledProject, GenerateError, GenerateOptions, assemble_bowl, validate_bowl,
};
pub use snapshot::{ArtifactFamily, GenerationSnapshot, SnapshotArtifact, SnapshotGroup};
