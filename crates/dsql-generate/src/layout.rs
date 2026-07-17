//! Build-tree layout (docs/spec/build-daemon.md, Transactionality):
//! content-addressed artifact files under per-family directories, an
//! immutable manifest per generation, and the fixed pointer.

use crate::snapshot::{ArtifactFamily, artifact_address, artifact_directory, artifact_file_stem};

pub(crate) const BUILD_DIR: &str = "build";
pub(crate) const MANIFEST_FILE: &str = "manifest.json";
pub(crate) const ARTIFACT_EXTENSION: &str = "json";

/// The immutable manifest file for one generation.
pub(crate) fn generation_manifest_file(generation_id: u64) -> String {
    format!("manifest.{generation_id}.json")
}

/// The content-addressed artifact path, relative to the build directory —
/// also the manifest entry's `path`. Distinct generations never overwrite
/// each other's files: the address is the artifact's own hash.
pub(crate) fn artifact_file_name(family: ArtifactFamily, name: &str, hash: &str) -> String {
    let directory = artifact_directory(family);
    format!(
        "{directory}/{}.{}.{ARTIFACT_EXTENSION}",
        artifact_file_stem(name),
        artifact_address(hash),
    )
}
