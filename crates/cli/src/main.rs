use clap::{Parser, Subcommand};
use dsql_core::{Catalog, SourceSnapshot};
use dsql_frontend::{AnalysisHost, collect_diagnostics};
use miette::{IntoDiagnostic, Result};
use sqlx::{Row, postgres::PgPoolOptions};
use std::path::{Path, PathBuf};

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
        file: PathBuf,
    },
    Lsp,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if matches!(args.command, Command::Lsp) {
        return dsql_lsp::run_stdio()
            .await
            .map_err(|err| miette::miette!("LSP server failed: {err}"));
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
            file,
        } => {
            let catalog = load_catalog_for_path(&file);
            let analysis = analyze_file_with_catalog(file.clone(), catalog.clone()).await?;
            let diagnostics = collect_diagnostics(&analysis);
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
            for query in &analysis.plan.queries {
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
                let project_start = file.parent().unwrap_or_else(|| Path::new("."));
                let project = dsql_project::Project::load_from(project_start)?;
                let pool = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&project.config.database_url)
                    .await
                    .map_err(|error| miette::miette!("failed to connect to database: {error}"))?;
                let mut output = String::from("{");
                for generated in generated_queries {
                    let exec_sql = format!("select ({})::text", generated.sql);
                    let row = sqlx::query(&exec_sql)
                        .fetch_one(&pool)
                        .await
                        .map_err(|error| miette::miette!("failed to execute SQL: {error}"))?;
                    let value = row
                        .try_get::<String, _>(0)
                        .map_err(|error| miette::miette!("failed to read JSON result: {error}"))?;
                    if output.len() > 1 {
                        output.push(',');
                    }
                    output.push('\n');
                    output.push_str("  ");
                    output.push_str(
                        &serde_json::to_string(&generated.output_name).into_diagnostic()?,
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
        Command::Lsp => unreachable!("handled before tracing subscriber initialization"),
    }
    Ok(())
}

async fn analyze_file(path: PathBuf) -> Result<dsql_frontend::AnalysisResult> {
    let catalog = load_catalog_for_path(&path);
    analyze_file_with_catalog(path, catalog).await
}

async fn analyze_file_with_catalog(
    path: PathBuf,
    catalog: Catalog,
) -> Result<dsql_frontend::AnalysisResult> {
    let text = std::fs::read_to_string(&path).into_diagnostic()?;
    let host = AnalysisHost::new();
    host.set_catalog(catalog);
    let file = host.create_file(SourceSnapshot::from_string(text));
    host.analyze(file)
        .await
        .ok_or_else(|| miette::miette!("analysis failed"))
}

fn load_catalog_for_path(path: &Path) -> Catalog {
    let project_start = path.parent().unwrap_or_else(|| Path::new("."));
    dsql_project::Project::try_load_from(project_start)
        .and_then(|project| project.load_catalog().ok())
        .unwrap_or_else(Catalog::hardcoded)
}

fn print_diagnostics(diagnostics: &[dsql_core::Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "{:?} {:?} {:?}: {}",
            diagnostic.source, diagnostic.severity, diagnostic.range, diagnostic.message
        );
    }
}
