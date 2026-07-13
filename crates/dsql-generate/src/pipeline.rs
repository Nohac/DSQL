//! The generate pipeline: settle the project bowl, scoop the derived
//! facts, assemble metadata, write the `build/` tree, and hand off to the
//! configured host generator.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bowl::{Entity, Query, Singleton};
use tokio::process::Command;

use dsql_core::entities::variable::VariableBinding;
use dsql_core::facts::{
    BelongsToFile, DefKey, Diagnostic, PlanKey, Severity, Span, arm_generate_demands,
};
use dsql_core::plan::{FragmentPlanFact, OperationSeed, QueryPlanFact};
use dsql_core::source::{BelongsToHost, FilePath, ResolutionScope, ScopeImports, SourceOffset};
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_project::{Project, ProjectError, open_analysis_bowl};

use crate::assemble::{
    FragmentInputs, OperationInputs, fragment_metadata, operation_metadata, source_path,
};
use crate::layout::{BUILD_DIR, artifact_collision_key};
use crate::publish::{
    ArtifactFamily, GenerationSnapshot, PublishedGeneration, SnapshotArtifact, SnapshotGroup,
    prune, publish, sha256_hex,
};

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("the project has {count} error diagnostics; fix them before generating:\n{details}")]
    LanguageDiagnostics { count: usize, details: String },
    #[error("failed to assemble `{name}`: {message}")]
    Assembly { name: String, message: String },
    #[error("failed to serialize `{name}`: {message}")]
    Serialize { name: String, message: String },
    #[error(transparent)]
    ArtifactCollision(Box<ArtifactCollision>),
    #[error(
        "artifact `{id}` addresses `{path}`, which exists with different contents; \
         refusing to overwrite"
    )]
    AddressCollision { path: String, id: String },
    #[error("another process held the publication lock past the wait bound")]
    PublicationLocked,
    #[error("internal failure: {0}")]
    Internal(String),
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to resolve {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("host generator {cmd:?} failed with {status}")]
    Generator { cmd: Vec<String>, status: String },
    #[error("failed to spawn host generator {cmd:?}: {source}")]
    Spawn {
        cmd: Vec<String>,
        source: std::io::Error,
    },
}

/// Two artifacts of one kind normalize to the same build path: writing
/// both would silently overwrite one and duplicate manifest entries.
#[derive(Debug, thiserror::Error)]
#[error("{kind} `{first}` ({first_source}) and `{second}` ({second_source}) both write `{path}`")]
pub struct ArtifactCollision {
    pub kind: &'static str,
    pub first: String,
    pub first_source: String,
    pub second: String,
    pub second_source: String,
    pub path: String,
}

impl GenerateError {
    pub(crate) fn named(mut self, artifact: &str) -> Self {
        if let GenerateError::Assembly { name, .. } = &mut self
            && name.is_empty()
        {
            *name = artifact.to_string();
        }
        self
    }
}

pub(crate) type Result<T> = std::result::Result<T, GenerateError>;

#[derive(Debug, Clone, Copy, Default)]
pub struct GenerateOptions {
    /// Bound nested collection relations at this many rows.
    pub collection_limit: Option<u64>,
}

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

/// Generates the project's `build/` tree and runs the configured host
/// generator command.
pub async fn generate_project(
    project: &Project,
    options: GenerateOptions,
) -> Result<GenerateOutput> {
    let bowl = open_analysis_bowl(project).await?;
    let assembled = assemble_project(&bowl, project, options).await?;
    let published = publish_snapshot(project, &assembled.snapshot).await?;
    let generator =
        run_host_generator(project, &assembled.project_root, &published.manifest_path).await;
    // One-shot generation prunes before exiting — even when the host
    // generator failed, since the generation itself committed. The
    // daemon prunes after responding. Best-effort either way.
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

/// Publishes a snapshot transactionally, off the async runtime (the
/// publication lock is a blocking OS lock).
pub async fn publish_snapshot(
    project: &Project,
    snapshot: &GenerationSnapshot,
) -> Result<PublishedGeneration> {
    let build_dir = project.root.join(BUILD_DIR);
    let snapshot = snapshot.clone();
    tokio::task::spawn_blocking(move || publish(&build_dir, &snapshot))
        .await
        .map_err(|_| GenerateError::Internal("publication task panicked".to_string()))?
}

/// Everything generation checks short of writing: language diagnostics,
/// per-artifact assembly, and build-path collisions. `dsql validate` runs
/// exactly this over its own bowl.
pub async fn validate_assembly(
    bowl: &bowl::Bowl,
    project: &Project,
    options: GenerateOptions,
) -> Result<()> {
    assemble_project(bowl, project, options).await.map(|_| ())
}

/// The assembled generation with its project root — the shared unit both
/// one-shot generation and the daemon publish and answer from.
pub struct AssembledProject {
    pub snapshot: GenerationSnapshot,
    pub project_root: PathBuf,
}

/// Assembles the settled bowl into a [`GenerationSnapshot`]: per-artifact
/// metadata with scope identity and full content hashes, per-scope group
/// closures, and the flat-namespace collision check.
pub async fn assemble_project(
    bowl: &bowl::Bowl,
    project: &Project,
    options: GenerateOptions,
) -> Result<AssembledProject> {
    let facts = collect_facts(bowl, options).await?;
    let catalog = project.load_catalog().await?;
    let project_root = project
        .root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.root.clone());

    let mut artifacts = Vec::new();
    for operation in &facts.operations {
        let bindings = facts
            .bindings
            .get(&operation.def)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let metadata = operation_metadata(
            &catalog,
            &project_root,
            &OperationInputs {
                seed: &operation.seed,
                plan: &operation.plan.0,
                sql: &operation.sql.0,
                bindings,
                file: &operation.file,
                source_offset: operation.source_offset,
            },
        )
        .map_err(|error| error.named(&operation.seed.query_name))?;
        artifacts.push(snapshot_artifact(
            ArtifactFamily::Operation,
            metadata.name.clone(),
            metadata.kind.clone(),
            &metadata,
            &operation.scope,
            &project_root,
            &operation.file,
        )?);
    }

    for fragment in &facts.fragments {
        let bindings = facts
            .bindings
            .get(&fragment.def)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let metadata = fragment_metadata(
            &catalog,
            &project_root,
            &FragmentInputs {
                plan: &fragment.plan,
                bindings,
                file: &fragment.file,
                source_offset: fragment.source_offset,
            },
        )?;
        artifacts.push(snapshot_artifact(
            ArtifactFamily::Fragment,
            metadata.name.clone(),
            metadata.kind.clone(),
            &metadata,
            &fragment.scope,
            &project_root,
            &fragment.file,
        )?);
    }
    artifacts.sort_by(|left, right| left.id.cmp(&right.id));

    validate_artifact_paths(&artifacts)?;

    // Per-scope groups: every scope that owns artifacts or appears in the
    // import graph, with its effective closure (own plus imported).
    let mut scope_names: std::collections::BTreeSet<String> =
        facts.imports.keys().cloned().collect();
    scope_names.extend(artifacts.iter().map(|artifact| artifact.scope.clone()));
    let groups = scope_names
        .into_iter()
        .map(|name| {
            let imports = facts.imports.get(&name).cloned().unwrap_or_default();
            let visible: std::collections::BTreeSet<&str> = std::iter::once(name.as_str())
                .chain(imports.iter().map(String::as_str))
                .collect();
            let members = artifacts
                .iter()
                .filter(|artifact| visible.contains(artifact.scope.as_str()))
                .map(|artifact| artifact.id.clone())
                .collect();
            SnapshotGroup {
                name,
                imports,
                artifacts: members,
            }
        })
        .collect();

    Ok(AssembledProject {
        snapshot: GenerationSnapshot { artifacts, groups },
        project_root,
    })
}

fn snapshot_artifact<M: facet::Facet<'static>>(
    family: ArtifactFamily,
    name: String,
    kind: String,
    metadata: &M,
    scope: &str,
    project_root: &Path,
    file: &str,
) -> Result<SnapshotArtifact> {
    let serialized = facet_json::to_string(metadata).map_err(|error| GenerateError::Serialize {
        name: name.clone(),
        message: error.to_string(),
    })?;
    let hash = sha256_hex(serialized.as_bytes());
    Ok(SnapshotArtifact {
        id: format!("{scope}/{}/{name}", family.label()),
        family,
        kind,
        scope: scope.to_string(),
        name,
        serialized,
        hash,
        source: source_path(project_root, file),
    })
}

/// Rejects artifact name collisions before anything touches the build
/// tree: two artifacts of one family whose names case-fold to the same
/// file stem would alias each other in the manifest's flat per-family
/// namespace (and on case-insensitive filesystems). Content addressing
/// removes *physical* overwrites, but the manifest still keys entries by
/// name, so same-scope duplicates stay language diagnostics and
/// cross-scope duplicates stay generate-boundary errors until the
/// scope-qualified layout lands (docs/issues.md).
fn validate_artifact_paths(artifacts: &[SnapshotArtifact]) -> Result<()> {
    let mut seen: HashMap<String, &SnapshotArtifact> = HashMap::new();
    for artifact in artifacts {
        let key = artifact_collision_key(artifact.family, &artifact.name);
        if let Some(first) = seen.insert(key.clone(), artifact) {
            return Err(GenerateError::ArtifactCollision(Box::new(
                ArtifactCollision {
                    kind: artifact.family.label(),
                    first: first.name.clone(),
                    first_source: first.source.clone(),
                    second: artifact.name.clone(),
                    second_source: artifact.source.clone(),
                    path: key,
                },
            )));
        }
    }
    Ok(())
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
    // The generator contract (docs/spec/codegen.md): cwd is the project
    // base, DSQL_PROJECT_DIR names it absolutely, and DSQL_MANIFEST
    // points at the manifest just written.
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

/// Everything scooped from the settled bowl, grouped for assembly.
struct CollectedFacts {
    operations: Vec<CollectedOperation>,
    fragments: Vec<CollectedFragment>,
    bindings: BTreeMap<u64, Vec<VariableBinding>>,
    /// The scope import graph, for group closures.
    imports: BTreeMap<String, Vec<String>>,
}

struct CollectedOperation {
    def: u64,
    seed: OperationSeed,
    plan: QueryPlanFact,
    sql: GeneratedSqlFact,
    file: String,
    scope: String,
    source_offset: usize,
}

struct CollectedFragment {
    def: u64,
    plan: FragmentPlanFact,
    file: String,
    scope: String,
    source_offset: usize,
}

async fn collect_facts(bowl: &bowl::Bowl, options: GenerateOptions) -> Result<CollectedFacts> {
    arm_generate_demands(bowl).await;
    if options.collection_limit.is_some() {
        bowl.insert((
            Singleton::<SqlOptions>::new(),
            SqlOptions {
                collection_limit: options.collection_limit,
            },
        ))
        .await;
    }

    // Error diagnostics fail generation with their details up front.
    let diagnostics = bowl
        .scoop::<Query<(Entity, &Severity, &Span, &Diagnostic, &BelongsToFile)>>()
        .await;
    // Documents are plain files (their own `FilePath`) or extracted
    // regions: those resolve to their host's path, and spans shift by
    // their offset into host coordinates.
    let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let path_rows = paths.collect();
    let regions = bowl
        .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
        .await;
    let region_rows = regions.collect();
    let path_of = |file: Entity| {
        let target = region_rows
            .iter()
            .find(|(entity, _, _)| *entity == file)
            .map_or(file, |(_, host, _)| host.0);
        path_rows
            .iter()
            .find(|(entity, _)| *entity == target)
            .map(|(_, path)| path.0.clone())
            .unwrap_or_default()
    };
    let offset_of = |file: Entity| {
        region_rows
            .iter()
            .find(|(entity, _, _)| *entity == file)
            .map(|(_, _, offset)| offset.0)
            .unwrap_or_default()
    };
    let scopes = bowl.scoop::<Query<(Entity, &ResolutionScope)>>().await;
    let scope_rows = scopes.collect();
    let scope_of = |file: Entity| {
        scope_rows
            .iter()
            .find(|(entity, _)| *entity == file)
            .map(|(_, scope)| scope.0.clone())
            .unwrap_or_default()
    };
    let errors: Vec<String> = diagnostics
        .collect()
        .into_iter()
        .filter(|(_, severity, _, _, _)| **severity == Severity::Error)
        .map(|(_, _, span, diagnostic, file)| {
            let offset = offset_of(file.0);
            format!(
                "{}:{}..{}: {}",
                path_of(file.0),
                offset + span.start,
                offset + span.end,
                diagnostic.0
            )
        })
        .collect();
    if !errors.is_empty() {
        return Err(GenerateError::LanguageDiagnostics {
            count: errors.len(),
            details: errors.join("\n"),
        });
    }

    let sql_rows = bowl
        .scoop::<Query<(Entity, &GeneratedSqlFact, &PlanKey)>>()
        .await;
    let sql_rows = sql_rows.collect();

    let plan_rows = bowl
        .scoop::<Query<(
            Entity,
            &QueryPlanFact,
            &OperationSeed,
            &PlanKey,
            &DefKey,
            &BelongsToFile,
        )>>()
        .await;
    let mut operations = Vec::new();
    for (_, plan, seed, plan_key, def, file) in plan_rows.collect() {
        // SQL failures surface as error diagnostics and fail generation
        // above; a missing pairing past that point is a bug, not a plan
        // to silently drop.
        let Some((_, sql, _)) = sql_rows
            .iter()
            .find(|(_, _, sql_key)| sql_key.0 == plan_key.0)
        else {
            return Err(GenerateError::Assembly {
                name: seed.query_name.clone(),
                message: "plan has no generated SQL".to_string(),
            });
        };
        operations.push(CollectedOperation {
            def: def.0.raw(),
            seed: seed.clone(),
            plan: plan.clone(),
            sql: (*sql).clone(),
            file: path_of(file.0),
            scope: scope_of(file.0),
            source_offset: offset_of(file.0),
        });
    }

    let fragment_rows = bowl
        .scoop::<Query<(Entity, &FragmentPlanFact, &DefKey, &BelongsToFile)>>()
        .await;
    let mut fragments = Vec::new();
    for (_, plan, def, file) in fragment_rows.collect() {
        fragments.push(CollectedFragment {
            def: def.0.raw(),
            plan: plan.clone(),
            file: path_of(file.0),
            scope: scope_of(file.0),
            source_offset: offset_of(file.0),
        });
    }

    let binding_rows = bowl
        .scoop::<Query<(Entity, &Span, &VariableBinding, &DefKey)>>()
        .await;
    let mut bindings: BTreeMap<u64, Vec<(Span, VariableBinding)>> = BTreeMap::new();
    for (_, span, binding, def) in binding_rows.collect() {
        bindings
            .entry(def.0.raw())
            .or_default()
            .push((*span, binding.clone()));
    }
    let bindings = bindings
        .into_iter()
        .map(|(def, mut rows)| {
            rows.sort_by_key(|(span, _)| (span.start, span.end));
            (def, rows.into_iter().map(|(_, binding)| binding).collect())
        })
        .collect();

    let imports = bowl
        .scoop::<Query<(Entity, &ScopeImports)>>()
        .await
        .collect()
        .into_iter()
        .next()
        .map(|(_, imports)| imports.0.clone())
        .unwrap_or_default();

    Ok(CollectedFacts {
        operations,
        fragments,
        bindings,
        imports,
    })
}
