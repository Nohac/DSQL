//! Build-tree layout (docs/spec/build-daemon.md, Transactionality):
//! content-addressed artifact files under per-family directories, an
//! immutable manifest per generation, and the fixed pointer.

use crate::publish::{ArtifactFamily, artifact_address};

pub(crate) const BUILD_DIR: &str = "build";
pub(crate) const MANIFEST_FILE: &str = "manifest.json";
pub(crate) const OPERATIONS_DIR: &str = "operations";
pub(crate) const FRAGMENTS_DIR: &str = "fragments";
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

/// The case-folded stem two artifacts of one kind may not share
/// (case-insensitive filesystems would alias them).
pub(crate) fn artifact_collision_key(family: ArtifactFamily, name: &str) -> String {
    let directory = artifact_directory(family);
    format!(
        "{directory}/{}",
        artifact_file_stem(name).to_ascii_lowercase()
    )
}

fn artifact_directory(family: ArtifactFamily) -> &'static str {
    match family {
        ArtifactFamily::Operation => OPERATIONS_DIR,
        ArtifactFamily::Fragment => FRAGMENTS_DIR,
    }
}

pub(crate) fn artifact_file_stem(name: &str) -> String {
    let mut output = String::new();
    for char in name.chars() {
        if char.is_ascii_alphanumeric() || char == '-' || char == '_' {
            output.push(char);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "operation".to_string()
    } else {
        output
    }
}
