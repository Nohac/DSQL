use dsql_metadata::{BuildManifest, OperationMetadata};
use miette::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactRef {
    pub path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct OperationArtifact {
    pub metadata: OperationMetadata,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WrittenOperationArtifact {
    pub metadata: OperationMetadata,
    pub reference: ArtifactRef,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WrittenArtifacts {
    pub manifest: ArtifactRef,
    pub operations: Vec<WrittenOperationArtifact>,
}

#[allow(async_fn_in_trait)]
pub(crate) trait ArtifactWriter {
    async fn write_operation(&self, operation: &OperationArtifact) -> Result<ArtifactRef>;

    async fn write_manifest(&self, manifest: &BuildManifest) -> Result<ArtifactRef>;
}
