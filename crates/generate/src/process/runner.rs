use miette::Result;
use tokio::process::Command;

use crate::{
    artifacts::WrittenArtifacts,
    runner::{GenerateTarget, GeneratorRunner},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CommandGeneratorRunner;

impl GeneratorRunner for CommandGeneratorRunner {
    async fn run(&self, target: &GenerateTarget, artifacts: &WrittenArtifacts) -> Result<()> {
        let Some(program) = target.cmd.first() else {
            return Ok(());
        };
        let status = Command::new(program)
            .args(&target.cmd[1..])
            .current_dir(&target.project_dir)
            .env("DSQL_PROJECT_DIR", &target.project_dir)
            .env("DSQL_MANIFEST", &artifacts.manifest.path)
            .env("DSQL_OUT_DIR", &target.out_dir)
            .status()
            .await
            .map_err(|error| miette::miette!("failed to run generator `{program}`: {error}"))?;
        if !status.success() {
            return Err(miette::miette!(
                "generator `{}` failed with status {}",
                target.cmd.join(" "),
                status
            ));
        }
        Ok(())
    }
}
