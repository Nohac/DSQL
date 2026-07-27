//! Storage-independent generation: settle a bowl and assemble artifact values.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bowl::{Entity, Query, Singleton};

use dsql_core::catalog::CatalogSnapshot;
use dsql_core::entities::policy::{CompiledPolicyIndex, PolicyDecl, PolicyIndex};
use dsql_core::entities::variable::{DefinitionVariables, VariableBinding};
use dsql_core::facts::{
    BelongsToFile, DefKey, Diagnostic, PlanKey, Severity, Span, arm_generate_demands,
};
use dsql_core::plan::{FragmentPlanFact, OperationSeed, QueryPlanFact};
use dsql_core::source::{
    BelongsToHost, ContentSpan, FilePath, ResolutionScope, ScopeImports, SourceOffset,
};
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_metadata::{DefinitionKind, SourceRange};

use crate::assemble::{
    FragmentInputs, OperationInputs, PolicySourceInput, fragment_metadata, operation_metadata,
    source_path,
};
use crate::match_lock::assemble_filter_match_lock;
use crate::snapshot::{
    ArtifactFamily, GenerationSnapshot, SnapshotArtifact, SnapshotGroup, artifact_collision_key,
    sha256_hex,
};

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("{0}")]
    Project(String),
    #[error("the project has {count} error diagnostics; fix them before generating:\n{details}")]
    LanguageDiagnostics { count: usize, details: String },
    #[error("failed to assemble `{name}`: {message}")]
    Assembly { name: String, message: String },
    #[error("failed to serialize `{name}`: {message}")]
    Serialize { name: String, message: String },
    #[error(
        "artifact `{id}` belongs to scope `{scope}`, which is absent from the configured graph"
    )]
    ArtifactScopeNotConfigured { id: String, scope: String },
    #[error(transparent)]
    ArtifactCollision(Box<ArtifactCollision>),
    #[error(
        "artifact `{id}` addresses `{path}`, which exists with different contents; \
         refusing to overwrite"
    )]
    AddressCollision { path: String, id: String },
    #[error("another process held the publication lock past the wait bound")]
    PublicationLocked,
    #[error("filter match lock `{path}` is not usable: {message}")]
    MatchLock { path: PathBuf, message: String },
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

/// Checks and assembles a settled in-memory bowl without writing artifacts.
pub async fn validate_bowl(
    bowl: &bowl::Bowl,
    source_root: Option<&Path>,
    options: GenerateOptions,
) -> Result<()> {
    assemble_bowl(bowl, source_root, options).await.map(|_| ())
}

/// The assembled generation snapshot shared by one-shot generation and
/// the daemon.
#[derive(Debug)]
pub struct AssembledProject {
    pub snapshot: GenerationSnapshot,
}

/// Assembles the settled bowl into a [`GenerationSnapshot`]: per-artifact
/// metadata with scope identity and full content hashes, per-scope group
/// closures, and the flat-namespace collision check.
pub async fn assemble_bowl(
    bowl: &bowl::Bowl,
    source_root: Option<&Path>,
    options: GenerateOptions,
) -> Result<AssembledProject> {
    let facts = collect_facts(bowl, options).await?;
    let catalog_rows = bowl.scoop::<Query<(Entity, &CatalogSnapshot)>>().await;
    let catalog_rows = catalog_rows.collect();
    let Some((_, catalog)) = catalog_rows.first() else {
        return Err(GenerateError::Internal(
            "language bowl has no catalog snapshot".to_string(),
        ));
    };
    let catalog = catalog.catalog();

    let mut artifacts = Vec::new();
    for operation in &facts.operations {
        let bindings = facts
            .bindings
            .get(&operation.def)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let metadata = operation_metadata(
            catalog,
            source_root,
            &OperationInputs {
                seed: &operation.seed,
                plan: &operation.plan.0,
                sql: &operation.sql.0,
                bindings,
                file: &operation.file,
                source_offset: operation.source_offset,
                content_range: operation.content_range,
                policy_sources: &facts.policy_sources,
            },
        )
        .map_err(|error| error.named(&operation.seed.query_name))?;
        artifacts.push(snapshot_artifact(
            ArtifactFamily::Operation,
            metadata.name.clone(),
            metadata.kind,
            &metadata,
            &operation.scope,
            source_root,
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
            catalog,
            source_root,
            &FragmentInputs {
                plan: &fragment.plan,
                bindings,
                file: &fragment.file,
                source_offset: fragment.source_offset,
                content_range: fragment.content_range,
            },
        )?;
        artifacts.push(snapshot_artifact(
            ArtifactFamily::Fragment,
            metadata.name.clone(),
            metadata.kind,
            &metadata,
            &fragment.scope,
            source_root,
            &fragment.file,
        )?);
    }
    artifacts.sort_by(|left, right| left.id.cmp(&right.id));

    validate_artifact_paths(&artifacts)?;

    // Per-scope groups: every scope that owns artifacts or appears in the
    // import graph, with its effective transitive closure.
    let scope_graph = if facts.imports.is_empty() {
        artifacts
            .iter()
            .map(|artifact| (artifact.scope.clone(), Vec::new()))
            .collect()
    } else {
        for artifact in &artifacts {
            if !facts.imports.contains_key(&artifact.scope) {
                return Err(GenerateError::ArtifactScopeNotConfigured {
                    id: artifact.id.clone(),
                    scope: artifact.scope.clone(),
                });
            }
        }
        facts.imports.clone()
    };
    let scope_imports = ScopeImports(scope_graph);
    let groups = scope_imports
        .0
        .keys()
        .cloned()
        .map(|name| {
            let imports = scope_imports.0.get(&name).cloned().unwrap_or_default();
            let visible: std::collections::BTreeSet<&str> =
                scope_imports.visible_from(&name).collect();
            let members = artifacts
                .iter()
                .filter(|artifact| visible.contains(artifact.scope.as_str()))
                .map(|artifact| artifact.id.clone())
                .collect();
            SnapshotGroup {
                generation_target: scope_imports.is_generation_target(&name),
                name,
                imports,
                artifacts: members,
            }
        })
        .collect();
    let project_contract = crate::ProjectContract::from_imports(&scope_imports)?;
    let filter_match_lock = assemble_filter_match_lock(
        catalog,
        &facts.policy_index,
        &facts.compiled_policies,
        &scope_imports,
    );

    Ok(AssembledProject {
        snapshot: GenerationSnapshot {
            artifacts,
            groups,
            project_contract,
            filter_match_lock,
        },
    })
}

fn snapshot_artifact<M: facet::Facet<'static>>(
    family: ArtifactFamily,
    name: String,
    kind: DefinitionKind,
    metadata: &M,
    scope: &str,
    source_root: Option<&Path>,
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
        source: source_path(source_root, file),
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

/// Everything scooped from the settled bowl, grouped for assembly.
struct CollectedFacts {
    operations: Vec<CollectedOperation>,
    fragments: Vec<CollectedFragment>,
    bindings: BTreeMap<u64, Vec<VariableBinding>>,
    /// The scope import graph, for group closures.
    imports: BTreeMap<String, Vec<String>>,
    policy_index: PolicyIndex,
    compiled_policies: CompiledPolicyIndex,
    policy_sources: Vec<PolicySourceInput>,
}

struct CollectedOperation {
    def: u64,
    seed: OperationSeed,
    plan: QueryPlanFact,
    sql: GeneratedSqlFact,
    file: String,
    scope: String,
    source_offset: usize,
    content_range: Option<SourceRange>,
}

struct CollectedFragment {
    def: u64,
    plan: FragmentPlanFact,
    file: String,
    scope: String,
    source_offset: usize,
    content_range: Option<SourceRange>,
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
        .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset, &ContentSpan)>>()
        .await;
    let region_rows = regions.collect();
    let path_of = |file: Entity| {
        let target = region_rows
            .iter()
            .find(|(entity, _, _, _)| *entity == file)
            .map_or(file, |(_, host, _, _)| host.0);
        path_rows
            .iter()
            .find(|(entity, _)| *entity == target)
            .map(|(_, path)| path.0.clone())
            .unwrap_or_default()
    };
    let offset_of = |file: Entity| {
        region_rows
            .iter()
            .find(|(entity, _, _, _)| *entity == file)
            .map(|(_, _, offset, _)| offset.0)
            .unwrap_or_default()
    };
    // Plain files have no region row and answer `None`: `content_range`
    // exists only for embedded documents.
    let content_of = |file: Entity| {
        region_rows
            .iter()
            .find(|(entity, _, _, _)| *entity == file)
            .map(|(_, _, _, content)| SourceRange {
                start: content.0.start as u32,
                end: content.0.end as u32,
            })
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
            content_range: content_of(file.0),
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
            content_range: content_of(file.0),
        });
    }

    let binding_rows = bowl
        .scoop::<Query<(Entity, &DefinitionVariables, &DefKey)>>()
        .await;
    let mut bindings: BTreeMap<u64, Vec<VariableBinding>> = BTreeMap::new();
    for (_, variables, def) in binding_rows.collect() {
        bindings.insert(def.0.raw(), variables.bindings.clone());
    }

    let imports = bowl
        .scoop::<Query<(Entity, &ScopeImports)>>()
        .await
        .collect()
        .into_iter()
        .next()
        .map(|(_, imports)| imports.0.clone())
        .unwrap_or_default();
    let policies = bowl
        .scoop::<Query<(Entity, &PolicyIndex, &CompiledPolicyIndex)>>()
        .await;
    let policies = policies.collect();
    let Some((_, policy_index, compiled_policies)) = policies.first() else {
        return Err(GenerateError::Internal(
            "language bowl has no compiled policy index".to_string(),
        ));
    };
    let policy_sources = bowl
        .scoop::<Query<(Entity, &PolicyDecl, &BelongsToFile)>>()
        .await
        .collect()
        .into_iter()
        .map(|(entity, declaration, file)| PolicySourceInput {
            entity,
            file: path_of(file.0),
            source_offset: offset_of(file.0),
            content_range: content_of(file.0),
            span: declaration.span,
        })
        .collect();

    Ok(CollectedFacts {
        operations,
        fragments,
        bindings,
        imports,
        policy_index: (*policy_index).clone(),
        compiled_policies: (*compiled_policies).clone(),
        policy_sources,
    })
}
