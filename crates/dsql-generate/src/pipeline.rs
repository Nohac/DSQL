//! The generate pipeline: settle the project bowl, scoop the derived
//! facts, assemble metadata, write the `build/` tree, and hand off to the
//! configured host generator.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use bowl::{Entity, Query, Singleton};
use futures::executor::block_on;

use dsql_core::entities::variable::VariableBinding;
use dsql_core::facts::{
    BelongsToFile, DefKey, Diagnostic, DiagnosticsDemand, PlanDemand, PlanKey, Severity, Span,
    SqlDemand, VariablesDemand,
};
use dsql_core::plan::{FragmentPlanFact, OperationSeed, QueryPlanFact};
use dsql_core::source::{FilePath, SourceOffset};
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_metadata::{
    BuildManifest, FragmentManifestEntry, FragmentMetadata, OperationManifestEntry,
    OperationMetadata,
};
use dsql_project::{Project, ProjectError, open_project_bowl};

use crate::assemble::{
    FragmentInputs, OperationInputs, fragment_metadata, operation_metadata, source_path,
    stable_hash,
};
use crate::layout::{
    BUILD_DIR, MANIFEST_FILE, fragment_artifact_path, fragment_manifest_path,
    operation_artifact_path, operation_manifest_path,
};

const BUILD_MANIFEST_VERSION: u32 = 1;

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
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("host generator {cmd:?} failed with {status}")]
    Generator { cmd: Vec<String>, status: String },
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
    pub manifest_path: PathBuf,
    /// Artifact files written this run (unchanged ones are skipped).
    pub written: Vec<PathBuf>,
}

/// Generates the project's `build/` tree and runs the configured host
/// generator command.
pub fn generate_project(project: &Project, options: GenerateOptions) -> Result<GenerateOutput> {
    let facts = block_on(collect_facts(project, options))?;
    let catalog = project.load_catalog()?;
    let project_root = project
        .root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.root.clone());

    let mut operations = Vec::new();
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
        operations.push(hashed(metadata, |metadata| &metadata.name, &project_root, &operation.file)?);
    }
    operations.sort_by(|left, right| left.0.name.cmp(&right.0.name));

    let mut fragments = Vec::new();
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
        fragments.push(hashed(metadata, |metadata| &metadata.name, &project_root, &fragment.file)?);
    }
    fragments.sort_by(|left, right| left.0.name.cmp(&right.0.name));

    write_build_tree(project, &project_root, operations, fragments)
}

/// One assembled artifact with its serialized form, content hash, and
/// project-relative source path.
struct Hashed<M>(M, String, String, String);

fn hashed<M: facet::Facet<'static>>(
    metadata: M,
    name: impl Fn(&M) -> &str,
    project_root: &Path,
    file: &str,
) -> Result<Hashed<M>> {
    let serialized =
        facet_json::to_string(&metadata).map_err(|error| GenerateError::Serialize {
            name: name(&metadata).to_string(),
            message: error.to_string(),
        })?;
    let hash = stable_hash(&serialized);
    let source = source_path(project_root, file);
    Ok(Hashed(metadata, serialized, hash, source))
}

fn write_build_tree(
    project: &Project,
    project_root: &Path,
    operations: Vec<Hashed<OperationMetadata>>,
    fragments: Vec<Hashed<FragmentMetadata>>,
) -> Result<GenerateOutput> {
    let build_dir = project.root.join(BUILD_DIR);
    let mut written = Vec::new();

    let mut operation_entries = Vec::new();
    for Hashed(metadata, serialized, hash, source) in &operations {
        let path = operation_artifact_path(&build_dir, &metadata.name);
        if write_if_changed(&path, serialized)? {
            written.push(path);
        }
        operation_entries.push(OperationManifestEntry {
            name: metadata.name.clone(),
            kind: metadata.kind.clone(),
            path: operation_manifest_path(&metadata.name),
            hash: hash.clone(),
            source: source.clone(),
        });
    }

    let mut fragment_entries = Vec::new();
    for Hashed(metadata, serialized, hash, source) in &fragments {
        let path = fragment_artifact_path(&build_dir, &metadata.name);
        if write_if_changed(&path, serialized)? {
            written.push(path);
        }
        fragment_entries.push(FragmentManifestEntry {
            name: metadata.name.clone(),
            kind: metadata.kind.clone(),
            path: fragment_manifest_path(&metadata.name),
            hash: hash.clone(),
            source: source.clone(),
        });
    }

    let manifest = BuildManifest {
        version: BUILD_MANIFEST_VERSION,
        operations: operation_entries,
        fragments: fragment_entries,
    };
    let manifest_path = build_dir.join(MANIFEST_FILE);
    let serialized =
        facet_json::to_string(&manifest).map_err(|error| GenerateError::Serialize {
            name: MANIFEST_FILE.to_string(),
            message: error.to_string(),
        })?;
    if write_if_changed(&manifest_path, &serialized)? {
        written.push(manifest_path.clone());
    }

    run_host_generator(project, project_root)?;

    Ok(GenerateOutput {
        manifest_path,
        written,
    })
}

/// Writes only when content changed: unchanged artifacts keep their mtime,
/// so downstream watchers and the host generator see per-operation deltas.
fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(path)
        && existing == content
    {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| GenerateError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, content).map_err(|source| GenerateError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(true)
}

fn run_host_generator(project: &Project, project_root: &Path) -> Result<()> {
    let typescript = &project.config.generate.typescript;
    if !typescript.enabled || typescript.cmd.is_empty() {
        return Ok(());
    }
    let status = Command::new(&typescript.cmd[0])
        .args(&typescript.cmd[1..])
        .current_dir(project_root)
        .status()
        .map_err(|source| GenerateError::Write {
            path: PathBuf::from(&typescript.cmd[0]),
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
}

struct CollectedOperation {
    def: u64,
    seed: OperationSeed,
    plan: QueryPlanFact,
    sql: GeneratedSqlFact,
    file: String,
    source_offset: usize,
}

struct CollectedFragment {
    def: u64,
    plan: FragmentPlanFact,
    file: String,
    source_offset: usize,
}

async fn collect_facts(project: &Project, options: GenerateOptions) -> Result<CollectedFacts> {
    let bowl = open_project_bowl(project).await?;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
        .await;
    bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
        .await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
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
    let paths = bowl.scoop::<Query<(Entity, &FilePath, &SourceOffset)>>().await;
    let path_rows = paths.collect();
    let path_of = |file: Entity| {
        path_rows
            .iter()
            .find(|(entity, _, _)| *entity == file)
            .map(|(_, path, _)| path.0.clone())
            .unwrap_or_default()
    };
    let offset_of = |file: Entity| {
        path_rows
            .iter()
            .find(|(entity, _, _)| *entity == file)
            .map(|(_, _, offset)| offset.0)
            .unwrap_or_default()
    };
    let errors: Vec<String> = diagnostics
        .collect()
        .into_iter()
        .filter(|(_, severity, _, _, _)| **severity == Severity::Error)
        .map(|(_, _, span, diagnostic, file)| {
            format!("{}:{}..{}: {}", path_of(file.0), span.start, span.end, diagnostic.0)
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
        let Some((_, sql, _)) = sql_rows
            .iter()
            .find(|(_, _, sql_key)| sql_key.0 == plan_key.0)
        else {
            continue;
        };
        operations.push(CollectedOperation {
            def: def.0.raw(),
            seed: seed.clone(),
            plan: plan.clone(),
            sql: (*sql).clone(),
            file: path_of(file.0),
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

    Ok(CollectedFacts {
        operations,
        fragments,
        bindings,
    })
}
