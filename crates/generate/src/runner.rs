use crate::GeneratorError;
use crate::artifacts::WrittenArtifacts;

#[derive(Clone, Debug)]
pub(crate) struct GenerateTarget {
    pub project_dir: String,
    pub cmd: Vec<String>,
}

#[allow(async_fn_in_trait)]
pub(crate) trait GeneratorRunner {
    async fn run(
        &self,
        target: &GenerateTarget,
        artifacts: &WrittenArtifacts,
    ) -> std::result::Result<(), GeneratorError>;
}
