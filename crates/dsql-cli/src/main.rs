//! The dsql command line: check, SQL generation, and formatting over the
//! project's bowl.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use dsql_cli::commands;

#[derive(Parser)]
#[command(name = "dsql", about = "The dsql compiler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new dsql project (dsql/dsql.toml plus schema/).
    Init {
        /// Base directory to scaffold into (defaults to the current one).
        path: Option<PathBuf>,
        /// Introspect this database into the fresh schema directory.
        #[arg(long)]
        database_url: Option<String>,
    },
    /// Type-check the project (or one file) and print diagnostics.
    Check {
        /// Narrow the report to this document's diagnostics.
        file: Option<PathBuf>,
    },
    /// Validate everything generation needs, without writing anything.
    Validate {
        /// Require the current resolved filter matches to equal dsql.lock.
        #[arg(long)]
        locked: bool,
    },
    /// Resolve filters and update only dsql/dsql.lock.
    Lock,
    /// Parse one file and print its lossless syntax tree.
    Parse { file: PathBuf },
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
        /// Format only this document.
        file: Option<PathBuf>,
    },
    /// Write the build/ artifact tree and run the host generator.
    Generate {
        /// What to generate.
        #[arg(long, value_enum, default_value_t = GenerateTarget::Project)]
        target: GenerateTarget,
        /// Bound nested collection relations at this many rows
        /// (project target only).
        #[arg(long)]
        collection_limit: Option<u64>,
        /// Output directory for the typescript-metadata target.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Require the current resolved filter matches to equal dsql.lock
        /// (project target only).
        #[arg(long)]
        locked: bool,
    },
    /// Print the build manifest's JSON Schema.
    MetadataSchema,
    /// Print the build manifest's TypeScript types.
    MetadataTypescript,
    /// Serve the build daemon protocol over stdio
    /// (docs/spec/build-daemon.md).
    Daemon {
        /// Require every compile to match dsql.lock without modifying it.
        #[arg(long)]
        locked: bool,
    },
    /// Introspect the project database into the schema/ directory.
    Introspect {
        /// Print the metadata as YAML instead of writing schema/.
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect or execute compiled operations.
    #[command(visible_alias = "op")]
    Operation {
        #[command(subcommand)]
        command: OperationCommand,
    },
    /// Debug introspection over the project bowl (debug builds only).
    #[cfg(debug_assertions)]
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Subcommand)]
enum OperationCommand {
    /// List compiled operations, optionally narrowed to one scope.
    List {
        #[arg(long)]
        scope: Option<String>,
    },
    /// Execute one compiled operation against the project database.
    Execute {
        name: String,
        #[arg(long)]
        scope: String,
        /// Inline JSON containing the `params` and `input` trees.
        #[arg(long, conflicts_with = "variables_file")]
        variables: Option<String>,
        /// Read the `params` and `input` JSON trees from this file.
        #[arg(long)]
        variables_file: Option<PathBuf>,
        /// Inline trusted server-context JSON, without a `context` wrapper.
        #[arg(long, conflicts_with = "context_file")]
        context: Option<String>,
        /// Read trusted server-context JSON from this file.
        #[arg(long)]
        context_file: Option<PathBuf>,
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
    /// Print the engine's explain report for a system.
    Explain { system: String },
}

/// The generate subcommand's output flavors.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum GenerateTarget {
    /// The build/ artifact tree plus the configured host generator.
    Project,
    /// The TypeScript consumer contract (manifest schema and types).
    TypescriptMetadata,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Init { path, database_url } => commands::init(path, database_url).await,
        Command::Check { file } => commands::check(file).await,
        Command::Validate { locked } => commands::validate(locked).await,
        Command::Lock => commands::lock().await,
        Command::Parse { file } => commands::parse_file(&file).await,
        Command::Sql { collection_limit } => commands::sql(collection_limit).await,
        Command::Fmt { check, file } => commands::fmt(check, file).await,
        Command::Generate {
            target: GenerateTarget::Project,
            collection_limit,
            out_dir: None,
            locked,
        } => commands::generate(collection_limit, locked).await,
        Command::Generate {
            target: GenerateTarget::Project,
            out_dir: Some(_),
            ..
        } => {
            eprintln!("error: --out-dir only applies to --target typescript-metadata");
            return std::process::ExitCode::FAILURE;
        }
        Command::Generate {
            target: GenerateTarget::TypescriptMetadata,
            locked: true,
            ..
        } => {
            eprintln!("error: --locked only applies to --target project");
            return std::process::ExitCode::FAILURE;
        }
        Command::Generate {
            target: GenerateTarget::TypescriptMetadata,
            collection_limit: Some(_),
            ..
        } => {
            eprintln!("error: --collection-limit only applies to --target project");
            return std::process::ExitCode::FAILURE;
        }
        Command::Generate {
            target: GenerateTarget::TypescriptMetadata,
            out_dir,
            ..
        } => {
            commands::generate_typescript_metadata(&out_dir.unwrap_or_else(|| PathBuf::from(".")))
                .await
        }
        Command::MetadataSchema => commands::metadata_schema(),
        Command::MetadataTypescript => commands::metadata_typescript(),
        Command::Daemon { locked } => {
            dsql_daemon::run_stdio(locked).await;
            return std::process::ExitCode::SUCCESS;
        }
        Command::Introspect { dry_run } => commands::introspect(dry_run).await,
        Command::Operation {
            command: OperationCommand::List { scope },
        } => commands::operation_list(scope.as_deref()).await,
        Command::Operation {
            command:
                OperationCommand::Execute {
                    name,
                    scope,
                    variables,
                    variables_file,
                    context,
                    context_file,
                },
        } => {
            commands::operation_execute(
                &scope,
                &name,
                variables.as_deref(),
                variables_file.as_deref(),
                context.as_deref(),
                context_file.as_deref(),
            )
            .await
        }
        #[cfg(debug_assertions)]
        Command::Debug { command } => match command {
            DebugCommand::Hover { file, offset } => dsql_cli::debug::hover(&file, offset).await,
            DebugCommand::Goto { file, offset } => dsql_cli::debug::goto(&file, offset).await,
            DebugCommand::Complete { file, offset } => {
                dsql_cli::debug::complete(&file, offset).await
            }
            DebugCommand::Tokens { file } => dsql_cli::debug::tokens(&file).await,
            DebugCommand::Resolution => dsql_cli::debug::resolution().await,
            DebugCommand::Explain { system } => dsql_cli::debug::explain(&system).await,
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
