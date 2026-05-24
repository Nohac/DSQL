use dsql_metadata::{BuildManifest, FragmentMetadata, OperationMetadata};
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
pub(crate) struct FragmentArtifact {
    pub metadata: FragmentMetadata,
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
pub(crate) struct WrittenFragmentArtifact {
    pub metadata: FragmentMetadata,
    pub reference: ArtifactRef,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WrittenArtifacts {
    pub manifest: ArtifactRef,
    pub operations: Vec<WrittenOperationArtifact>,
    pub fragments: Vec<WrittenFragmentArtifact>,
}

#[allow(async_fn_in_trait)]
pub(crate) trait ArtifactWriter {
    async fn write_operation(&self, operation: &OperationArtifact) -> Result<ArtifactRef>;

    async fn write_fragment(&self, fragment: &FragmentArtifact) -> Result<ArtifactRef>;

    async fn write_manifest(&self, manifest: &BuildManifest) -> Result<ArtifactRef>;
}
