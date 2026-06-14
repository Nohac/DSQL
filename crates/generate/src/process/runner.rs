use tokio::process::Command;

use crate::{
    GeneratorError,
    artifacts::WrittenArtifacts,
    runner::{GenerateTarget, GeneratorRunner},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CommandGeneratorRunner;

impl GeneratorRunner for CommandGeneratorRunner {
    async fn run(
        &self,
        target: &GenerateTarget,
        artifacts: &WrittenArtifacts,
    ) -> std::result::Result<(), GeneratorError> {
        let Some(program) = target.cmd.first() else {
            return Ok(());
        };
        let _ = artifacts.fragments.len();
        let status = Command::new(program)
            .args(&target.cmd[1..])
            .current_dir(&target.project_dir)
            .env("DSQL_PROJECT_DIR", &target.project_dir)
            .env("DSQL_MANIFEST", &artifacts.manifest.path)
            .status()
            .await
            .map_err(|source| GeneratorError::Spawn {
                program: program.clone(),
                source,
            })?;
        if !status.success() {
            return Err(GeneratorError::Failed {
                command: target.cmd.join(" "),
                status,
            });
        }
        Ok(())
    }
}
