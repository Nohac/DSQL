use clap::{Parser, Subcommand};
use dsql_core::SourceSnapshot;
use dsql_frontend::{analyze_source, collect_diagnostics};
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
    Parse { file: PathBuf },
    Check { file: PathBuf },
    Fmt { file: PathBuf },
    Lsp,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    match args.command {
        Command::Parse { file } => {
            let analysis = analyze_file(file)?;
            print!("{}", analysis.parse.tree);
            print_diagnostics(&collect_diagnostics(&analysis));
        }
        Command::Check { file } => {
            let analysis = analyze_file(file)?;
            print_diagnostics(&collect_diagnostics(&analysis));
        }
        Command::Fmt { file } => {
            let analysis = analyze_file(file)?;
            let formatted = dsql_core::format_file(&analysis.parse);
            for diagnostic in &formatted.diagnostics {
                eprintln!(
                    "{:?} {:?}: {}",
                    diagnostic.source, diagnostic.range, diagnostic.message
                );
            }
            print!("{}", formatted.text);
        }
        Command::Lsp => dsql_lsp::run_stdio()
            .await
            .map_err(|err| miette::miette!("LSP server failed: {err}"))?,
    }
    Ok(())
}

fn analyze_file(path: PathBuf) -> Result<dsql_frontend::AnalysisResult> {
    let text = std::fs::read_to_string(path).into_diagnostic()?;
    Ok(analyze_source(SourceSnapshot::from_string(text)))
}

fn print_diagnostics(diagnostics: &[dsql_core::Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "{:?} {:?} {:?}: {}",
            diagnostic.source, diagnostic.severity, diagnostic.range, diagnostic.message
        );
    }
}
