use clap::{Parser, Subcommand, ValueEnum};
use dsql_core::{
    Catalog, CompilerDiagnostic, DefinitionRecord, DsqlDiagnostic, FragmentMap, QueryRecord,
    SourceSnapshot, check_query_definition, collect_compiler_diagnostic_sources,
    collect_query_compiler_diagnostics, extract_definitions, lint_query_definition_with_options,
    parse_source, plan_query_definition, sort_compiler_diagnostics,
};
use dsql_frontend::{PhysicalDocumentId, ProjectHost};
use miette::{IntoDiagnostic, NamedSource, Result};
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
    Validate,
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
            let analysis = analyze_file(file.clone()).await?;
            print!("{}", analysis.parse.tree);
            print_analysis_diagnostics(&file, &analysis);
        }
        Command::Check { file } => {
            let analysis = analyze_file(file.clone()).await?;
            print_analysis_diagnostics(&file, &analysis);
        }
        Command::Fmt { file } => {
            let analysis = analyze_file(file.clone()).await?;
            let formatted = dsql_core::format_file(&analysis.parse);
            for diagnostic in &formatted.diagnostics {
                print_miette_diagnostic(
                    file.display().to_string(),
                    analysis.parse.source.to_arc_str().to_string(),
                    diagnostic.clone(),
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
            let mut diagnostics = query.parse_diagnostics.clone();
            diagnostics.extend(collect_query_compiler_diagnostics(
                &checked, &linted, &planned,
            ));
            sort_compiler_diagnostics(&mut diagnostics);
            for diagnostic in &diagnostics {
                print_miette_diagnostic(
                    query.source_name.clone(),
                    query.source_text.clone(),
                    diagnostic.clone(),
                );
            }
            if diagnostics
                .iter()
                .any(|diagnostic| matches!(diagnostic.severity(), dsql_core::Severity::Error))
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
        Command::Validate => {
            let validation =
                dsql_generate::validate_project_from(&std::env::current_dir().into_diagnostic()?)
                    .await?;
            print_validation_output(&validation);
            if validation
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.severity == dsql_core::Severity::Error)
            {
                return Err(miette::miette!("dsql validation failed"));
            }
        }
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
    parse_diagnostics: Vec<CompilerDiagnostic>,
    source_name: String,
    source_text: String,
}

fn load_project_query(project: &dsql_project::Project, query_name: &str) -> Result<LoadedQuery> {
    let documents = dsql_project::load_project_documents(project)?;
    if documents.is_empty() {
        return Err(miette::miette!(
            "no dsql documents found in project {}",
            dsql_project::project_base(project).display()
        ));
    }

    let mut fragments = FragmentMap::default();
    let mut queries = Vec::<QueryRecord>::new();
    let mut parse_diagnostics = Vec::<CompilerDiagnostic>::new();
    let mut matched_source_name = None::<String>;
    let mut matched_source_text = None::<String>;

    for document in documents {
        let source_name = document.path.display().to_string();
        let source_text = document.text;
        let parsed = parse_source(SourceSnapshot::from_string(source_text.clone()));
        parse_diagnostics.extend(parsed.diagnostics.clone().into_iter().map(Into::into));
        let extracted = extract_definitions(&parsed.source_file);
        for definition in extracted.definitions {
            match definition {
                DefinitionRecord::Query(query) => {
                    if query.key.name.as_deref() == Some(query_name) {
                        matched_source_name.get_or_insert_with(|| source_name.clone());
                        matched_source_text.get_or_insert_with(|| source_text.clone());
                    }
                    queries.push(query);
                }
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
            source_name: matched_source_name.unwrap_or_else(|| "<dsql>".to_string()),
            source_text: matched_source_text.unwrap_or_default(),
        }),
        _ => Err(miette::miette!(
            "query `{query_name}` is defined multiple times"
        )),
    }
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
    let host = ProjectHost::new();
    host.set_standalone_context("file");
    host.set_catalog(catalog);
    host.set_lint_options(lint_options);
    let document_id = PhysicalDocumentId(path.clone());
    host.open_document(document_id.clone(), Some(path), 0, text);
    host.analysis_for_document(&document_id)
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

fn print_analysis_diagnostics(path: &Path, analysis: &dsql_frontend::AnalysisResult) {
    let source_name = path.display().to_string();
    let source_text = analysis.parse.source.to_arc_str().to_string();
    let diagnostics = collect_compiler_diagnostic_sources(&[
        &analysis.parse,
        &analysis.lower,
        &analysis.check,
        &analysis.lint,
        &analysis.plan,
    ]);
    for diagnostic in diagnostics {
        print_miette_diagnostic(source_name.clone(), source_text.clone(), diagnostic);
    }
}

fn print_miette_diagnostic(
    source_name: String,
    source_text: String,
    diagnostic: impl miette::Diagnostic + Send + Sync + 'static,
) {
    let source = NamedSource::new(source_name, source_text).with_language("dsql");
    eprintln!(
        "{:?}",
        miette::Report::new(diagnostic).with_source_code(source)
    );
}

fn print_validation_output(validation: &dsql_generate::ValidationOutput) {
    for diagnostic in &validation.diagnostics {
        let path = diagnostic
            .path
            .as_ref()
            .unwrap_or(&diagnostic.physical_document.0);
        let source_text = std::fs::read_to_string(path).unwrap_or_default();
        print_miette_diagnostic(
            path.display().to_string(),
            source_text,
            diagnostic.diagnostic.clone(),
        );
    }
    if validation.diagnostics.is_empty() {
        println!(
            "validated {} document(s), {} quer{}",
            validation.document_count,
            validation.query_count,
            if validation.query_count == 1 {
                "y"
            } else {
                "ies"
            }
        );
    }
}
