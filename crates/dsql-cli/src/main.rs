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
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Check => commands::check(),
        Command::Sql { collection_limit } => commands::sql(collection_limit),
        Command::Fmt { check } => commands::fmt(check),
        Command::Generate { collection_limit } => commands::generate(collection_limit),
        Command::Introspect => commands::introspect(),
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
