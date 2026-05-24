use clap::{Parser, Subcommand, ValueEnum};
use dsql_core::{
    Catalog, DefinitionRecord, Diagnostic, FragmentMap, QueryRecord, SourceSnapshot,
    check_query_definition, extract_definitions, lint_query_definition_with_options, parse_source,
    plan_query_definition,
};
use dsql_frontend::{AnalysisHost, collect_diagnostics};
use miette::{IntoDiagnostic, Result};
use sqlx::{Connection, Row, postgres::PgConnection};
use std::path::{Path, PathBuf};

mod daemon;

#[derive(Parser)]
#[command(name = "dsql")]
#[command(about = "dsql language tools")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init {
        path: Option<PathBuf>,
        #[arg(short, long)]
        database_url: Option<String>,
    },
    Introspect {
        #[arg(short, long)]
        dry_run: bool,
    },
    Parse {
        file: PathBuf,
    },
    Check {
        file: PathBuf,
    },
    Fmt {
        file: PathBuf,
    },
    Exec {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value_t = 100)]
        limit: u64,
        query: String,
    },
    Generate {
        #[arg(long, value_enum, default_value_t = GenerateTarget::Project)]
        target: GenerateTarget,
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    MetadataSchema,
    MetadataTypescript,
    Daemon,
    Lsp,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GenerateTarget {
    Project,
    TypescriptMetadata,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if matches!(args.command, Command::Lsp) {
        return dsql_lsp::run_stdio()
            .await
            .map_err(|err| miette::miette!("LSP server failed: {err}"));
    }
    if matches!(args.command, Command::Daemon) {
        return daemon::run_stdio().await;
    }

    tracing_subscriber::fmt::init();
    match args.command {
        Command::Init { path, database_url } => {
            let base = path.unwrap_or_else(|| PathBuf::from("."));
            let project = dsql_project::init_project(&base, database_url.clone())?;
            if let Some(database_url) = database_url {
                let metadata = dsql_introspection::introspect_postgres(&database_url)
                    .await
                    .map_err(|error| miette::miette!("failed to introspect database: {error}"))?;
                dsql_project::store_metadata_dir(&metadata, &project.schema)?;
            }
        }
        Command::Introspect { dry_run } => {
            let project = dsql_project::Project::load()?;
            let metadata = dsql_introspection::introspect_postgres(&project.config.database_url)
                .await
                .map_err(|error| miette::miette!("failed to introspect database: {error}"))?;
            if dry_run {
                print!(
                    "{}",
                    dsql_core::metadata_to_yaml(&metadata).map_err(|error| miette::miette!(
                        "failed to serialize database metadata: {error}"
                    ))?
                );
            } else {
                dsql_project::store_metadata_dir(&metadata, &project.schema)?;
            }
        }
        Command::Parse { file } => {
            let analysis = analyze_file(file).await?;
            print!("{}", analysis.parse.tree);
            print_diagnostics(&collect_diagnostics(&analysis));
        }
        Command::Check { file } => {
            let analysis = analyze_file(file).await?;
            print_diagnostics(&collect_diagnostics(&analysis));
        }
        Command::Fmt { file } => {
            let analysis = analyze_file(file).await?;
            let formatted = dsql_core::format_file(&analysis.parse);
            for diagnostic in &formatted.diagnostics {
                eprintln!(
                    "{:?} {:?}: {}",
                    diagnostic.source, diagnostic.range, diagnostic.message
                );
            }
            print!("{}", formatted.text);
        }
        Command::Exec {
            dry_run,
            limit,
            query,
        } => {
            let project = dsql_project::Project::load()?;
            let catalog = project.load_catalog()?;
            let lint_options = project.lint_options();
            let query = load_project_query(&project, &query)?;

            let checked = check_query_definition(&query.record, &query.fragments, &catalog);
            let linted = lint_query_definition_with_options(
                &query.record,
                &query.fragments,
                &catalog,
                lint_options,
            );
            let planned = plan_query_definition(&query.record, &query.fragments, &catalog);
            let mut diagnostics = Vec::new();
            diagnostics.extend(query.parse_diagnostics);
            diagnostics.extend(checked.diagnostics);
            diagnostics.extend(linted.diagnostics);
            diagnostics.extend(planned.diagnostics);
            diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
            print_diagnostics(&diagnostics);
            if diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic.severity, dsql_core::Severity::Error))
            {
                return Err(miette::miette!(
                    "cannot generate SQL while diagnostics contain errors"
                ));
            }
            let mut generated_queries = Vec::new();
            let sql_options = dsql_core::PostgresSqlOptions {
                collection_limit: (limit > 0).then_some(limit),
            };
            for query in &planned.queries {
                generated_queries.push(
                    dsql_core::generate_postgres_sql_with_options(query, &catalog, sql_options)
                        .map_err(|error| miette::miette!("failed to generate SQL: {error}"))?,
                );
            }

            if dry_run {
                for generated in generated_queries {
                    print!("{}", generated.sql);
                    if !generated.sql.ends_with('\n') {
                        println!();
                    }
                }
            } else {
                let mut connection = PgConnection::connect(&project.config.database_url)
                    .await
                    .map_err(|error| miette::miette!("failed to connect to database: {error}"))?;
                let backend_pid = sqlx::query("select pg_backend_pid()")
                    .fetch_one(&mut connection)
                    .await
                    .and_then(|row| row.try_get::<i32, _>(0))
                    .map_err(|error| {
                        miette::miette!("failed to read database backend pid: {error}")
                    })?;
                let mut output = String::from("{");
                for generated in generated_queries {
                    let exec_sql = format!("select ({})::text", generated.sql);
                    let row = tokio::select! {
                        row = sqlx::query(&exec_sql).fetch_one(&mut connection) => {
                            row.map_err(|error| miette::miette!("failed to execute SQL: {error}"))?
                        }
                        signal = tokio::signal::ctrl_c() => {
                            signal
                                .map_err(|error| miette::miette!("failed to listen for Ctrl+C: {error}"))?;
                            cancel_backend(&project.config.database_url, backend_pid).await?;
                            return Err(miette::miette!("query execution cancelled"));
                        }
                    };
                    let value = row
                        .try_get::<String, _>(0)
                        .map_err(|error| miette::miette!("failed to read JSON result: {error}"))?;
                    if output.len() > 1 {
                        output.push(',');
                    }
                    output.push('\n');
                    output.push_str("  ");
                    output.push_str(
                        &facet_json::to_string(&generated.output_name).into_diagnostic()?,
                    );
                    output.push_str(": ");
                    output.push_str(&value);
                }
                if output.len() > 1 {
                    output.push('\n');
                }
                output.push('}');
                println!("{output}");
            }
        }
        Command::Generate { target, out_dir } => match target {
            GenerateTarget::Project => {
                let output = dsql_generate::generate_project_from(
                    &std::env::current_dir().into_diagnostic()?,
                )
                .await?;
                println!("wrote {}", output.manifest_path);
                for file in output.operation_paths {
                    println!("wrote {file}");
                }
            }
            GenerateTarget::TypescriptMetadata => {
                let out_dir = out_dir.unwrap_or_else(|| PathBuf::from("."));
                generate_typescript_metadata(&out_dir).await?;
            }
        },
        Command::MetadataSchema => {
            let schema = dsql_metadata::build_manifest_json_schema();
            println!("{schema}");
        }
        Command::MetadataTypescript => {
            print!("{}", dsql_metadata::build_manifest_typescript());
        }
        Command::Daemon => unreachable!("handled before tracing subscriber initialization"),
        Command::Lsp => unreachable!("handled before tracing subscriber initialization"),
    }
    Ok(())
}

async fn generate_typescript_metadata(out_dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(out_dir).await.map_err(|error| {
        miette::miette!(
            "failed to create generated output directory {}: {error}",
            out_dir.display()
        )
    })?;

    tokio::fs::write(
        out_dir.join("build-manifest.schema.json"),
        dsql_metadata::build_manifest_json_schema(),
    )
    .await
    .map_err(|error| miette::miette!("failed to write build manifest schema: {error}"))?;

    tokio::fs::write(
        out_dir.join("metadata.ts"),
        dsql_metadata::build_manifest_typescript(),
    )
    .await
    .map_err(|error| miette::miette!("failed to write TypeScript metadata types: {error}"))?;
    Ok(())
}

async fn cancel_backend(database_url: &str, backend_pid: i32) -> Result<()> {
    let mut connection = PgConnection::connect(database_url)
        .await
        .map_err(|error| miette::miette!("failed to connect for query cancellation: {error}"))?;
    sqlx::query("select pg_cancel_backend($1)")
        .bind(backend_pid)
        .execute(&mut connection)
        .await
        .map_err(|error| miette::miette!("failed to cancel database query: {error}"))?;
    Ok(())
}

struct LoadedQuery {
    record: QueryRecord,
    fragments: FragmentMap,
    parse_diagnostics: Vec<Diagnostic>,
}

fn load_project_query(project: &dsql_project::Project, query_name: &str) -> Result<LoadedQuery> {
    let files = project_document_files(project)?;
    if files.is_empty() {
        return Err(miette::miette!(
            "no dsql documents found in project {}",
            project_root(project).display()
        ));
    }

    let mut fragments = FragmentMap::default();
    let mut queries = Vec::<QueryRecord>::new();
    let mut parse_diagnostics = Vec::<Diagnostic>::new();

    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|error| miette::miette!("failed to read {}: {error}", file.display()))?;
        let parsed = parse_source(SourceSnapshot::from_string(text));
        parse_diagnostics.extend(parsed.diagnostics.clone());
        let extracted = extract_definitions(&parsed.source_file);
        for definition in extracted.definitions {
            match definition {
                DefinitionRecord::Query(query) => queries.push(query),
                DefinitionRecord::Fragment(fragment) => fragments.insert(fragment),
            }
        }
    }

    let matching = queries
        .into_iter()
        .filter(|query| query.key.name.as_deref() == Some(query_name))
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [] => Err(miette::miette!("query `{query_name}` not found")),
        [query] => Ok(LoadedQuery {
            record: query.clone(),
            fragments,
            parse_diagnostics,
        }),
        _ => Err(miette::miette!(
            "query `{query_name}` is defined multiple times"
        )),
    }
}

fn project_document_files(project: &dsql_project::Project) -> Result<Vec<PathBuf>> {
    let base = project_root(project);
    let mut files = Vec::new();
    if project.config.documents.is_empty() {
        collect_dsql_files(&base, Some(&project.root), &mut files)?;
    } else {
        for document in &project.config.documents {
            if document.resolver != "dsql" {
                continue;
            }
            for path in &document.paths {
                let path = base.join(path);
                collect_document_path(&path, Some(&project.root), &mut files)?;
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn project_root(project: &dsql_project::Project) -> PathBuf {
    project
        .root
        .parent()
        .map_or_else(|| project.root.clone(), Path::to_path_buf)
}

fn collect_document_path(
    path: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if path.is_dir() {
        collect_dsql_files(path, excluded_dir, files)
    } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
        files.push(path.to_path_buf());
        Ok(())
    } else {
        Err(miette::miette!(
            "dsql document path not found: {}",
            path.display()
        ))
    }
}

fn collect_dsql_files(
    dir: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if excluded_dir.is_some_and(|excluded| dir == excluded) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|error| miette::miette!("failed to read directory {}: {error}", dir.display()))?
    {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        if path.is_dir() {
            collect_dsql_files(&path, excluded_dir, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path);
        }
    }
    Ok(())
}

async fn analyze_file(path: PathBuf) -> Result<dsql_frontend::AnalysisResult> {
    let (catalog, lint_options) = load_project_settings_for_path(&path);
    analyze_file_with_settings(path, catalog, lint_options).await
}

async fn analyze_file_with_settings(
    path: PathBuf,
    catalog: Catalog,
    lint_options: dsql_core::LintOptions,
) -> Result<dsql_frontend::AnalysisResult> {
    let text = std::fs::read_to_string(&path).into_diagnostic()?;
    let host = AnalysisHost::new();
    host.set_catalog(catalog);
    host.set_lint_options(lint_options);
    let file = host.create_file(SourceSnapshot::from_string(text));
    host.analyze(file)
        .await
        .ok_or_else(|| miette::miette!("analysis failed"))
}

fn load_project_settings_for_path(path: &Path) -> (Catalog, dsql_core::LintOptions) {
    let project_start = path.parent().unwrap_or_else(|| Path::new("."));
    let Some(project) = dsql_project::Project::try_load_from(project_start) else {
        return (Catalog::hardcoded(), dsql_core::LintOptions::default());
    };
    let lint_options = project.lint_options();
    let catalog = project
        .load_catalog()
        .unwrap_or_else(|_| Catalog::hardcoded());
    (catalog, lint_options)
}

fn print_diagnostics(diagnostics: &[dsql_core::Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "{:?} {:?} {:?}: {}",
            diagnostic.source, diagnostic.severity, diagnostic.range, diagnostic.message
        );
    }
}
