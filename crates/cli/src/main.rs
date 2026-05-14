use clap::{Parser, Subcommand};
use dsql_core::SourceSnapshot;
use dsql_frontend::{AnalysisHost, collect_diagnostics};
use miette::{IntoDiagnostic, Result};
use std::path::PathBuf;

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
        Command::Lsp => unreachable!("handled before tracing subscriber initialization"),
    }
    Ok(())
}

async fn analyze_file(path: PathBuf) -> Result<dsql_frontend::AnalysisResult> {
    let text = std::fs::read_to_string(&path).into_diagnostic()?;
    let host = AnalysisHost::new();
    let project_start = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    if let Some(project) = dsql_project::Project::try_load_from(project_start)
        && let Ok(catalog) = project.load_catalog()
    {
        host.set_catalog(catalog);
    }
    let file = host.create_file(SourceSnapshot::from_string(text));
    host.analyze(file)
        .await
        .ok_or_else(|| miette::miette!("analysis failed"))
}

fn print_diagnostics(diagnostics: &[dsql_core::Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "{:?} {:?} {:?}: {}",
            diagnostic.source, diagnostic.severity, diagnostic.range, diagnostic.message
        );
    }
}
