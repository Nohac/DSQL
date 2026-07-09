//! The dsql command line: check, SQL generation, and formatting over the
//! project's bowl.

use clap::{Parser, Subcommand};
use dsql_cli::commands;

#[derive(Parser)]
#[command(name = "dsql", about = "The dsql compiler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Type-check every project document and print diagnostics.
    Check,
    /// Generate PostgreSQL for every query in the project.
    Sql {
        /// Bound nested collection relations at this many rows.
        #[arg(long)]
        collection_limit: Option<u64>,
    },
    /// Format `.dsql` files in place (or check with --check).
    Fmt {
        /// Report files that would change without rewriting them.
        #[arg(long)]
        check: bool,
    },
    /// Write the build/ artifact tree and run the host generator.
    Generate {
        /// Bound nested collection relations at this many rows.
        #[arg(long)]
        collection_limit: Option<u64>,
    },
    /// Introspect the project database into the schema/ directory.
    Introspect,
    /// Debug introspection over the project bowl (debug builds only).
    #[cfg(debug_assertions)]
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[cfg(debug_assertions)]
#[derive(Subcommand)]
enum DebugCommand {
    /// Answer a hover request and dump the facts at the offset.
    Hover { file: String, offset: usize },
    /// Answer a go-to-definition request.
    Goto { file: String, offset: usize },
    /// Answer a completion request.
    Complete { file: String, offset: usize },
    /// Dump a file's semantic tokens.
    Tokens { file: String },
    /// Dump file-to-scope ownership, derived regions, and scope imports.
    Resolution,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Check => commands::check().await,
        Command::Sql { collection_limit } => commands::sql(collection_limit).await,
        Command::Fmt { check } => commands::fmt(check).await,
        Command::Generate { collection_limit } => commands::generate(collection_limit).await,
        Command::Introspect => commands::introspect().await,
        #[cfg(debug_assertions)]
        Command::Debug { command } => match command {
            DebugCommand::Hover { file, offset } => dsql_cli::debug::hover(&file, offset).await,
            DebugCommand::Goto { file, offset } => dsql_cli::debug::goto(&file, offset).await,
            DebugCommand::Complete { file, offset } => {
                dsql_cli::debug::complete(&file, offset).await
            }
            DebugCommand::Tokens { file } => dsql_cli::debug::tokens(&file).await,
            DebugCommand::Resolution => dsql_cli::debug::resolution().await,
        },
    };
    match outcome {
        Ok(clean) if clean => std::process::ExitCode::SUCCESS,
        Ok(_) => std::process::ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
