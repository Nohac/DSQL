use dsql_metadata::BuildManifest;
use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};

use crate::artifacts::{ArtifactRef, ArtifactWriter, OperationArtifact};

#[derive(Clone, Debug)]
pub struct FsArtifactWriter {
    build_dir: PathBuf,
}

impl FsArtifactWriter {
    pub fn new(build_dir: impl Into<PathBuf>) -> Self {
        Self {
            build_dir: build_dir.into(),
        }
    }

    pub fn build_dir(&self) -> &Path {
        &self.build_dir
    }

    fn operation_path(&self, operation: &OperationArtifact) -> PathBuf {
        self.build_dir.join("operations").join(format!(
            "{}.json",
            artifact_file_stem(&operation.metadata.name)
        ))
    }
}

impl ArtifactWriter for FsArtifactWriter {
    async fn write_operation(&self, operation: &OperationArtifact) -> Result<ArtifactRef> {
        let path = self.operation_path(operation);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.into_diagnostic()?;
        }
        write_json(&path, &operation.metadata).await?;
        Ok(ArtifactRef {
            path: path
                .strip_prefix(&self.build_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string(),
        })
    }

    async fn write_manifest(&self, manifest: &BuildManifest) -> Result<ArtifactRef> {
        tokio::fs::create_dir_all(&self.build_dir)
            .await
            .into_diagnostic()?;
        let path = self.build_dir.join("manifest.json");
        write_json(&path, manifest).await?;
        Ok(ArtifactRef {
            path: path.to_string_lossy().to_string(),
        })
    }
}

async fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let json = serde_json::to_string_pretty(value).into_diagnostic()?;
    tokio::fs::write(path, format!("{json}\n"))
        .await
        .map_err(|error| miette::miette!("failed to write {}: {error}", path.display()))?;
    Ok(())
}

fn artifact_file_stem(name: &str) -> String {
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
