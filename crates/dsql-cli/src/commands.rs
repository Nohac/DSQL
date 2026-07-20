//! Command implementations over the project bowl. All async: the binary
//! owns the runtime, libraries and commands only do async work.

use std::path::{Path, PathBuf};

use bowl::{Bowl, Entity, Query, Singleton};

use dsql_core::catalog::{DatabaseMetadata, metadata_to_yaml};
use dsql_core::entities::document::ParsedFile;
use dsql_core::facts::{arm_editor_demands, arm_generate_demands};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::grammar::parser::{Node, NodeRef, Rule};
use dsql_core::source::{FilePath, SourceKind};
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_generate::publish::MatchLockMode;
use dsql_generate::{ArtifactFamily, SnapshotArtifact};
use dsql_metadata::OperationMetadata;
use dsql_project::{Project, ProjectError, load_project_documents, open_analysis_bowl};
use serde_json::Value;

/// The command layer's error: each failure keeps its own type instead of
/// being coerced into an unrelated project variant.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Generate(#[from] dsql_generate::GenerateError),
    #[error(transparent)]
    Introspection(#[from] dsql_introspection::IntrospectionError),
    #[error(transparent)]
    Execute(#[from] dsql_execute::ExecuteError),
    #[error("failed to parse JSON for {input}: {message}")]
    Json { input: String, message: String },
    #[error("{0} must be supplied inline or from a file, not both")]
    ConflictingJsonSources(String),
    #[error("operation `{scope}::{name}` was not found")]
    OperationNotFound { scope: String, name: String },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize metadata: {0}")]
    Metadata(String),
    #[error("{0} is not a project document")]
    NotAProjectDocument(PathBuf),
    #[error("{0} is an embedding host; format the extracted dsql regions via their editor")]
    FormatsHost(PathBuf),
}

pub(crate) type Outcome = Result<bool, CliError>;

fn effective_database_url(project: &Project) -> String {
    std::env::var("DSQL_DATABASE_URL").unwrap_or_else(|_| project.config.database_url.clone())
}

/// Resolves a user-supplied path onto the project document it names — the
/// exact path string diagnostics report under. Errors when the file is
/// not part of the project.
async fn project_member(bowl: &Bowl, file: &Path) -> Result<String, CliError> {
    let canonical = tokio::fs::canonicalize(file)
        .await
        .map_err(|source| CliError::Read {
            path: file.to_path_buf(),
            source,
        })?;
    let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    paths
        .collect()
        .into_iter()
        .map(|(_, path)| path.0.clone())
        .find(|path| std::fs::canonicalize(path).is_ok_and(|candidate| candidate == canonical))
        .ok_or_else(|| CliError::NotAProjectDocument(file.to_path_buf()))
}

/// Prints every diagnostic in the project (or just `file`'s, including
/// those projected from its embedded regions) as a miette report with its
/// source excerpt, sorted by file and span. Returns true when nothing is
/// an error.
pub async fn check(file: Option<PathBuf>) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = open_analysis_bowl(&project).await?;
        arm_editor_demands(&bowl).await;
        let selected = match &file {
            Some(file) => Some(project_member(&bowl, file).await?),
            None => None,
        };

        let diagnostics = crate::render::collect_diagnostics(&bowl).await;
        let shown: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| {
                selected
                    .as_deref()
                    .is_none_or(|path| diagnostic.path() == path)
            })
            .collect();
        for diagnostic in &shown {
            print!("{}", crate::render::render(diagnostic));
        }
        if shown.is_empty() {
            println!("no diagnostics");
        }
        Ok(!shown.iter().any(|diagnostic| diagnostic.is_error()))
    }
}

/// Counts `query` definitions in a parsed document, including malformed
/// and unnamed ones the definition facts would skip.
fn count_query_defs(cst: &dsql_core::grammar::parser::CstData) -> usize {
    let mut count = 0;
    let mut stack = vec![NodeRef::ROOT];
    while let Some(node) = stack.pop() {
        if let Node::Rule(rule, _) = cst.get(node) {
            if rule == Rule::QueryDef {
                count += 1;
            }
            stack.extend(cst.children(node));
        }
    }
    count
}

/// Everything generation would do short of writing: prints all
/// diagnostics, counts documents and queries, and dry-runs artifact
/// assembly (path collisions included). Fails only on errors — warnings
/// and infos report without failing the build.
pub async fn validate(locked: bool) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = open_analysis_bowl(&project).await?;
        arm_generate_demands(&bowl).await;

        let diagnostics = crate::render::collect_diagnostics(&bowl).await;
        for diagnostic in &diagnostics {
            print!("{}", crate::render::render(diagnostic));
        }
        let errors = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .count();

        let parsed = bowl.scoop::<Query<(Entity, &ParsedFile)>>().await;
        let rows = parsed.collect();
        let documents = rows.len();
        let queries: usize = rows
            .iter()
            .map(|(_, parsed)| count_query_defs(&parsed.cst))
            .sum();
        drop(rows);
        drop(parsed);

        if errors == 0 {
            // Language facts are clean; the remaining failure modes are
            // assembly ones (artifact path collisions, metadata mapping).
            let assembled =
                dsql_generate::assemble_project(&bowl, &project, Default::default()).await?;
            if locked {
                dsql_generate::reconcile_project_match_lock(
                    &project,
                    &assembled.snapshot.filter_match_lock,
                    MatchLockMode::Locked,
                )
                .await?;
            }
        }
        println!(
            "{documents} document{}, {queries} quer{}",
            if documents == 1 { "" } else { "s" },
            if queries == 1 { "y" } else { "ies" },
        );
        Ok(errors == 0)
    }
}

/// Prints the generated PostgreSQL for every query plan in the project.
pub async fn sql(collection_limit: Option<u64>) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = open_analysis_bowl(&project).await?;
        arm_generate_demands(&bowl).await;
        if collection_limit.is_some() {
            bowl.insert((
                Singleton::<SqlOptions>::new(),
                SqlOptions { collection_limit },
            ))
            .await;
        }

        // SQL for a project with errors is silently wrong; refuse it.
        let diagnostics = crate::render::collect_diagnostics(&bowl).await;
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .collect();
        if !errors.is_empty() {
            for diagnostic in errors {
                print!("{}", crate::render::render(diagnostic));
            }
            return Ok(false);
        }

        let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        let mut generated: Vec<(String, String)> = rows
            .collect()
            .into_iter()
            .map(|(_, fact)| (fact.0.output_name.clone(), fact.0.sql.clone()))
            .collect();
        generated.sort();

        for (name, sql) in &generated {
            println!("-- {name}\n{sql}\n");
        }
        Ok(true)
    }
}

/// Lists generation-clean operations without publishing artifacts.
pub async fn operation_list(scope: Option<&str>) -> Outcome {
    let project = Project::load().await?;
    let Some(operations) = compiled_operations(&project).await? else {
        return Ok(false);
    };
    for operation in operations
        .iter()
        .filter(|operation| scope.is_none_or(|scope| operation.scope == scope))
    {
        println!("{}\t{}", operation.scope, operation.name);
    }
    Ok(true)
}

/// Executes one generation-clean operation against the configured database.
pub async fn operation_execute(
    scope: &str,
    name: &str,
    variables: Option<&str>,
    variables_file: Option<&Path>,
    context: Option<&str>,
    context_file: Option<&Path>,
) -> Outcome {
    let project = Project::load().await?;
    let Some(operations) = compiled_operations(&project).await? else {
        return Ok(false);
    };
    let artifact = operations
        .iter()
        .find(|artifact| artifact.scope == scope && artifact.name == name)
        .ok_or_else(|| CliError::OperationNotFound {
            scope: scope.to_string(),
            name: name.to_string(),
        })?;
    let operation: OperationMetadata =
        facet_json::from_str(&artifact.serialized).map_err(|error| CliError::Json {
            input: artifact.id.clone(),
            message: error.to_string(),
        })?;
    let bindings = dsql_execute::ExecutionBindings {
        variables: json_input(variables, variables_file, "variables").await?,
        context: json_input(context, context_file, "context").await?,
    };
    let materialized = dsql_execute::materialize(&operation, &bindings)?;
    let executor =
        dsql_execute::PostgresExecutor::connect(&effective_database_url(&project)).await?;
    let output = executor.execute_materialized(&materialized).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| CliError::Json {
            input: "operation output".to_string(),
            message: error.to_string(),
        })?
    );
    Ok(true)
}

async fn compiled_operations(project: &Project) -> Result<Option<Vec<SnapshotArtifact>>, CliError> {
    let bowl = open_analysis_bowl(project).await?;
    arm_generate_demands(&bowl).await;
    let diagnostics = crate::render::collect_diagnostics(&bowl).await;
    let errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_error())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        for diagnostic in errors {
            print!("{}", crate::render::render(diagnostic));
        }
        return Ok(None);
    }
    let assembled = dsql_generate::assemble_project(&bowl, project, Default::default()).await?;
    Ok(Some(
        assembled
            .snapshot
            .artifacts
            .into_iter()
            .filter(|artifact| artifact.family == ArtifactFamily::Operation)
            .collect(),
    ))
}

async fn json_input(
    inline: Option<&str>,
    file: Option<&Path>,
    label: &str,
) -> Result<Value, CliError> {
    if inline.is_some() && file.is_some() {
        return Err(CliError::ConflictingJsonSources(label.to_string()));
    }
    let (input, raw) = if let Some(inline) = inline {
        (format!("inline {label}"), inline.to_string())
    } else if let Some(file) = file {
        (
            file.display().to_string(),
            tokio::fs::read_to_string(file)
                .await
                .map_err(|source| CliError::Read {
                    path: file.to_path_buf(),
                    source,
                })?,
        )
    } else {
        return Ok(serde_json::json!({}));
    };
    serde_json::from_str(&raw).map_err(|error| CliError::Json {
        input,
        message: error.to_string(),
    })
}

/// Formats project documents in place (or just `file`); with `check`,
/// reports instead. Returns true when nothing needed changing.
pub async fn fmt(check_only: bool, file: Option<PathBuf>) -> Outcome {
    let project = Project::load().await?;
    fmt_project(&project, check_only, file).await
}

/// [`fmt`] against an explicit project root.
pub async fn fmt_project(project: &Project, check_only: bool, only: Option<PathBuf>) -> Outcome {
    let documents = load_project_documents(project).await?;
    let only = match only {
        Some(file) => {
            let canonical =
                tokio::fs::canonicalize(&file)
                    .await
                    .map_err(|source| CliError::Read {
                        path: file.clone(),
                        source,
                    })?;
            let document = documents
                .iter()
                .find(|document| {
                    std::fs::canonicalize(&document.path)
                        .is_ok_and(|candidate| candidate == canonical)
                })
                .ok_or_else(|| CliError::NotAProjectDocument(file.clone()))?;
            if matches!(document.kind, SourceKind::Embedded(_)) {
                return Err(CliError::FormatsHost(file));
            }
            Some(canonical)
        }
        None => None,
    };

    let mut clean = true;
    for document in documents {
        if let Some(only) = &only
            && !std::fs::canonicalize(&document.path).is_ok_and(|candidate| candidate == *only)
        {
            continue;
        }
        // Host sources carry embedded regions; whole-file formatting is a
        // dsql-document affair until region-granular edits are supported.
        if matches!(document.kind, SourceKind::Embedded(_)) {
            continue;
        }
        let (cst, diagnostics) = parse(&document.text);
        let formatted = format_document(&cst.into_data(), &document.text, !diagnostics.is_empty());
        if formatted.confidence == FormatConfidence::PreserveOriginal {
            eprintln!("{}: skipped (parse errors)", document.path.display());
            clean = false;
            continue;
        }
        if formatted.text == document.text {
            continue;
        }
        if check_only {
            println!("{}: would reformat", document.path.display());
            clean = false;
        } else {
            tokio::fs::write(&document.path, formatted.text)
                .await
                .map_err(|source| ProjectError::Write {
                    path: document.path.clone(),
                    source,
                })?;
            println!("{}: reformatted", document.path.display());
        }
    }
    Ok(clean)
}

/// Writes the artifact tree and runs the configured host generator.
pub async fn generate(collection_limit: Option<u64>, locked: bool) -> Outcome {
    let project = Project::load().await?;
    let output = dsql_generate::generate_project(
        &project,
        dsql_generate::GenerateOptions { collection_limit },
        if locked {
            MatchLockMode::Locked
        } else {
            MatchLockMode::Update
        },
    )
    .await?;
    for path in &output.written {
        println!("{}: written", path.display());
    }
    println!("manifest: {}", output.manifest_path.display());
    Ok(true)
}

/// Resolves and atomically updates only `dsql/dsql.lock`.
pub async fn lock() -> Outcome {
    let project = Project::load().await?;
    let bowl = open_analysis_bowl(&project).await?;
    let assembled = dsql_generate::assemble_project(&bowl, &project, Default::default()).await?;
    let status = dsql_generate::reconcile_project_match_lock(
        &project,
        &assembled.snapshot.filter_match_lock,
        MatchLockMode::Update,
    )
    .await?;
    let path = project.root.join("dsql.lock");
    if status.changed {
        if status.content_hash.is_some() {
            println!("{}: updated", path.display());
        } else {
            println!("{}: removed", path.display());
        }
    } else {
        println!("{}: unchanged", path.display());
    }
    Ok(true)
}

/// Introspects the configured database; dry runs print the metadata as
/// one YAML document instead of writing the schema directory.
pub async fn introspect(dry_run: bool) -> Outcome {
    let project = Project::load().await?;
    let metadata =
        dsql_introspection::introspect_postgres(&effective_database_url(&project)).await?;
    match sink_metadata(&metadata, &project.schema, dry_run).await? {
        Some(rendered) => print!("{rendered}"),
        None => println!("schema written to {}", project.schema.display()),
    }
    Ok(true)
}

/// The introspection sink: dry runs render monolithic YAML and touch
/// nothing; real runs write the schema directory and return `None`.
pub async fn sink_metadata(
    metadata: &DatabaseMetadata,
    schema_dir: &Path,
    dry_run: bool,
) -> Result<Option<String>, CliError> {
    if dry_run {
        return metadata_to_yaml(metadata)
            .map(Some)
            .map_err(CliError::Metadata);
    }
    dsql_project::store_metadata_dir(metadata, schema_dir).await?;
    Ok(None)
}

/// Scaffolds a new project; with a database URL, introspects it into the
/// fresh schema directory immediately.
pub async fn init(path: Option<PathBuf>, database_url: Option<String>) -> Outcome {
    let base = path.unwrap_or_else(|| PathBuf::from("."));
    let project = dsql_project::init_project(&base, database_url.clone()).await?;
    println!("initialized {}", project.root.display());
    if let Some(database_url) = database_url {
        let metadata = dsql_introspection::introspect_postgres(&database_url).await?;
        sink_metadata(&metadata, &project.schema, false).await?;
        println!("schema written to {}", project.schema.display());
    }
    Ok(true)
}

/// Parses one file and prints its lossless CST, with parse diagnostics
/// after a `---` separator.
pub async fn parse_file(file: &Path) -> Outcome {
    let text = tokio::fs::read_to_string(file)
        .await
        .map_err(|source| CliError::Read {
            path: file.to_path_buf(),
            source,
        })?;
    let (cst, diagnostics) = parse(&text);
    print!("{cst}");
    for diagnostic in &diagnostics {
        println!("---");
        println!("{}", diagnostic.message);
    }
    Ok(diagnostics.is_empty())
}

/// Prints the build manifest's JSON Schema.
pub fn metadata_schema() -> Outcome {
    let schema = dsql_metadata::build_manifest_json_schema().map_err(CliError::Metadata)?;
    println!("{schema}");
    Ok(true)
}

/// Prints the build manifest's TypeScript type definitions.
pub fn metadata_typescript() -> Outcome {
    print!("{}", dsql_metadata::build_manifest_typescript());
    Ok(true)
}

/// Writes the TypeScript-consumer metadata contract — the build manifest's
/// JSON Schema and its TypeScript types — into `out_dir`.
pub async fn generate_typescript_metadata(out_dir: &Path) -> Outcome {
    tokio::fs::create_dir_all(out_dir)
        .await
        .map_err(|source| CliError::Write {
            path: out_dir.to_path_buf(),
            source,
        })?;
    let schema_path = out_dir.join("build-manifest.schema.json");
    let schema = dsql_metadata::build_manifest_json_schema().map_err(CliError::Metadata)?;
    tokio::fs::write(&schema_path, format!("{schema}\n"))
        .await
        .map_err(|source| CliError::Write {
            path: schema_path.clone(),
            source,
        })?;
    let types_path = out_dir.join("metadata.ts");
    tokio::fs::write(&types_path, dsql_metadata::build_manifest_typescript())
        .await
        .map_err(|source| CliError::Write {
            path: types_path.clone(),
            source,
        })?;
    println!("wrote {}", schema_path.display());
    println!("wrote {}", types_path.display());
    Ok(true)
}
