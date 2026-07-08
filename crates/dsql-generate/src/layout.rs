use std::path::{Path, PathBuf};

pub(crate) const BUILD_DIR: &str = "build";
pub(crate) const MANIFEST_FILE: &str = "manifest.json";
pub(crate) const OPERATIONS_DIR: &str = "operations";
pub(crate) const FRAGMENTS_DIR: &str = "fragments";
pub(crate) const ARTIFACT_EXTENSION: &str = "json";

pub(crate) fn operation_artifact_path(build_dir: &Path, name: &str) -> PathBuf {
    artifact_path(build_dir, OPERATIONS_DIR, name)
}

pub(crate) fn fragment_artifact_path(build_dir: &Path, name: &str) -> PathBuf {
    artifact_path(build_dir, FRAGMENTS_DIR, name)
}

pub(crate) fn operation_manifest_path(name: &str) -> String {
    artifact_manifest_path(OPERATIONS_DIR, name)
}

pub(crate) fn fragment_manifest_path(name: &str) -> String {
    artifact_manifest_path(FRAGMENTS_DIR, name)
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

fn artifact_path(build_dir: &Path, directory: &str, name: &str) -> PathBuf {
    build_dir.join(directory).join(format!(
        "{}.{}",
        artifact_file_stem(name),
        ARTIFACT_EXTENSION
    ))
}

fn artifact_manifest_path(directory: &str, name: &str) -> String {
    format!(
        "{}/{}.{}",
        directory,
        artifact_file_stem(name),
        ARTIFACT_EXTENSION
    )
}
