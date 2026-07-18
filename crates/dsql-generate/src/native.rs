//! Native project loading, filesystem publication, and host-generator adapter.

use std::path::{Path, PathBuf};

use dsql_project::{Project, ProjectError, open_analysis_bowl};
use tokio::process::Command;

use crate::layout::BUILD_DIR;
use crate::pipeline::{AssembledProject, GenerateError, GenerateOptions, Result, assemble_bowl};
use crate::publish::{
    MatchLockMode, MatchLockStatus, PublishedGeneration, prune, publish, reconcile_match_lock,
};
use crate::snapshot::GenerationSnapshot;

const MATCH_LOCK_FILE: &str = "dsql.lock";

/// Filesystem paths committed by one native generation.
#[derive(Debug, Clone)]
pub struct GenerateOutput {
    /// The committed generation.
    pub generation_id: u64,
    /// The immutable `manifest.<id>.json` this run committed.
    pub manifest_path: PathBuf,
    /// The fixed `manifest.json` pointer.
    pub current_manifest_path: PathBuf,
    /// Files written this run (unchanged artifacts are skipped).
    pub written: Vec<PathBuf>,
}

/// Generates the project's build tree and runs its configured host generator.
pub async fn generate_project(
    project: &Project,
    options: GenerateOptions,
    lock_mode: MatchLockMode,
) -> Result<GenerateOutput> {
    let bowl = open_analysis_bowl(project).await.map_err(project_error)?;
    let assembled = assemble_project(&bowl, project, options).await?;
    let published = publish_snapshot(project, &assembled.snapshot, lock_mode).await?;
    let generator = run_host_generator(project, project.base(), &published.manifest_path).await;
    // A committed generation is pruned even when the host generator fails.
    let build_dir = project.root.join(BUILD_DIR);
    {
        let published = published.clone();
        tokio::task::spawn_blocking(move || prune(&build_dir, &published))
            .await
            .ok();
    }
    generator?;
    Ok(GenerateOutput {
        generation_id: published.generation_id,
        manifest_path: published.manifest_path,
        current_manifest_path: published.current_manifest_path,
        written: published.written,
    })
}

/// Native adapter around [`assemble_bowl`] preserving project-relative paths.
pub async fn assemble_project(
    bowl: &bowl::Bowl,
    project: &Project,
    options: GenerateOptions,
) -> Result<AssembledProject> {
    assemble_bowl(bowl, Some(project.base()), options).await
}

/// Everything native validation checks short of writing artifacts.
pub async fn validate_assembly(
    bowl: &bowl::Bowl,
    project: &Project,
    options: GenerateOptions,
) -> Result<()> {
    assemble_project(bowl, project, options).await.map(|_| ())
}

/// Publishes a snapshot transactionally off the async runtime.
pub async fn publish_snapshot(
    project: &Project,
    snapshot: &GenerationSnapshot,
    lock_mode: MatchLockMode,
) -> Result<PublishedGeneration> {
    let build_dir = project.root.join(BUILD_DIR);
    let match_lock_path = project.root.join(MATCH_LOCK_FILE);
    let snapshot = snapshot.clone();
    tokio::task::spawn_blocking(move || publish(&build_dir, &match_lock_path, &snapshot, lock_mode))
        .await
        .map_err(|_| GenerateError::Internal("publication task panicked".to_string()))?
}

/// Updates or validates only the project's match lock under the publication
/// advisory lock, without writing artifact generations.
pub async fn reconcile_project_match_lock(
    project: &Project,
    lock: &crate::FilterMatchLock,
    mode: MatchLockMode,
) -> Result<MatchLockStatus> {
    let build_dir = project.root.join(BUILD_DIR);
    let match_lock_path = project.root.join(MATCH_LOCK_FILE);
    let lock = lock.clone();
    tokio::task::spawn_blocking(move || {
        reconcile_match_lock(&build_dir, &match_lock_path, &lock, mode)
    })
    .await
    .map_err(|_| GenerateError::Internal("match-lock task panicked".to_string()))?
}

fn project_error(error: ProjectError) -> GenerateError {
    GenerateError::Project(error.to_string())
}

async fn run_host_generator(
    project: &Project,
    project_root: &Path,
    manifest_path: &Path,
) -> Result<()> {
    let typescript = &project.config.generate.typescript;
    if !typescript.enabled || typescript.cmd.is_empty() {
        return Ok(());
    }
    let absolute_root = std::path::absolute(project_root).map_err(|source| GenerateError::Io {
        path: project_root.to_path_buf(),
        source,
    })?;
    let absolute_manifest =
        std::path::absolute(manifest_path).map_err(|source| GenerateError::Io {
            path: manifest_path.to_path_buf(),
            source,
        })?;
    let status = Command::new(&typescript.cmd[0])
        .args(&typescript.cmd[1..])
        .env("DSQL_PROJECT_DIR", &absolute_root)
        .env("DSQL_MANIFEST", &absolute_manifest)
        .current_dir(project_root)
        .status()
        .await
        .map_err(|source| GenerateError::Spawn {
            cmd: typescript.cmd.clone(),
            source,
        })?;
    if !status.success() {
        return Err(GenerateError::Generator {
            cmd: typescript.cmd.clone(),
            status: status.to_string(),
        });
    }
    Ok(())
}
