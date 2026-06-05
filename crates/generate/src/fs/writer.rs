use camino::Utf8Path;
use dsql_metadata::BuildManifest;
use std::path::{Path, PathBuf};

use crate::ArtifactError;
use crate::artifacts::{ArtifactRef, ArtifactWriter, FragmentArtifact, OperationArtifact};
use crate::layout::{fragment_artifact_path, manifest_path, operation_artifact_path};

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
        operation_artifact_path(&self.build_dir, &operation.metadata.name)
    }

    fn fragment_path(&self, fragment: &FragmentArtifact) -> PathBuf {
        fragment_artifact_path(&self.build_dir, &fragment.metadata.name)
    }

    async fn write_build_artifact<T>(
        &self,
        path: PathBuf,
        metadata: &T,
    ) -> std::result::Result<ArtifactRef, ArtifactError>
    where
        T: facet::Facet<'static>,
    {
        if let Some(parent) = path.parent() {
            create_dir_all(parent).await?;
        }
        write_json(&path, metadata).await?;
        let relative = path.strip_prefix(&self.build_dir).unwrap_or(&path);
        artifact_ref_from_path(relative)
    }
}

impl ArtifactWriter for FsArtifactWriter {
    async fn write_operation(
        &self,
        operation: &OperationArtifact,
    ) -> std::result::Result<ArtifactRef, ArtifactError> {
        self.write_build_artifact(self.operation_path(operation), &operation.metadata)
            .await
    }

    async fn write_fragment(
        &self,
        fragment: &FragmentArtifact,
    ) -> std::result::Result<ArtifactRef, ArtifactError> {
        self.write_build_artifact(self.fragment_path(fragment), &fragment.metadata)
            .await
    }

    async fn write_manifest(
        &self,
        manifest: &BuildManifest,
    ) -> std::result::Result<ArtifactRef, ArtifactError> {
        create_dir_all(&self.build_dir).await?;
        let path = manifest_path(&self.build_dir);
        write_json(&path, manifest).await?;
        artifact_ref_from_path(&path)
    }
}

fn artifact_ref_from_path(path: &Path) -> std::result::Result<ArtifactRef, ArtifactError> {
    let path = Utf8Path::from_path(path).ok_or_else(|| ArtifactError::NonUtf8Path {
        path: path.to_path_buf(),
    })?;
    Ok(ArtifactRef {
        path: path.as_str().to_string(),
    })
}

async fn write_json<T>(path: &Path, value: &T) -> std::result::Result<(), ArtifactError>
where
    T: facet::Facet<'static>,
{
    let json = facet_json::to_string_pretty(value)
        .map_err(|error| ArtifactError::SerializeJson(error.to_string()))?;
    tokio::fs::write(path, format!("{json}\n"))
        .await
        .map_err(|source| ArtifactError::WriteFile {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

async fn create_dir_all(path: &Path) -> std::result::Result<(), ArtifactError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|source| ArtifactError::CreateDir {
            path: path.to_path_buf(),
            source,
        })
}
