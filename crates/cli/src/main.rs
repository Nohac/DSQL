use clap::{Parser, Subcommand, ValueEnum};
use dsql_core::{Catalog, collect_compiler_diagnostic_sources};
use dsql_frontend::{PhysicalDocumentId, ProjectHost};
use miette::{
    IntoDiagnostic, MietteError, MietteSpanContents, Result, SourceCode, SourceSpan, SpanContents,
};
use ropey::{LineType, Rope};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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
                    RopeDiagnosticSource::new(
                        file.display().to_string(),
                        analysis.parse.source.arc_rope(),
                    ),
                    diagnostic.clone(),
                );
            }
            print!("{}", formatted.text);
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
    let diagnostics = collect_compiler_diagnostic_sources(&[
        &analysis.parse,
        &analysis.lower,
        &analysis.check,
        &analysis.lint,
        &analysis.plan,
    ]);
    let source = RopeDiagnosticSource::new(source_name, analysis.parse.source.arc_rope());
    for diagnostic in diagnostics {
        print_miette_diagnostic(source.clone(), diagnostic);
    }
}

fn print_miette_diagnostic(
    source: RopeDiagnosticSource,
    diagnostic: impl miette::Diagnostic + Send + Sync + 'static,
) {
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
        print_miette_diagnostic(
            RopeDiagnosticSource::from_path(path),
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

#[derive(Clone, Debug)]
struct RopeDiagnosticSource {
    name: String,
    rope: Arc<Rope>,
    buffers: Arc<Mutex<Vec<Box<[u8]>>>>,
}

impl RopeDiagnosticSource {
    fn new(name: String, rope: Arc<Rope>) -> Self {
        Self {
            name,
            rope,
            buffers: Arc::default(),
        }
    }

    fn from_path(path: &Path) -> Self {
        let rope = File::open(path)
            .ok()
            .and_then(|file| Rope::from_reader(file).ok())
            .unwrap_or_default();
        Self::new(path.display().to_string(), Arc::new(rope))
    }

    fn cache_bytes(&self, bytes: Vec<u8>) -> &[u8] {
        let mut buffers = self
            .buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = buffers.len();
        buffers.push(bytes.into_boxed_slice());
        let cached = &buffers[index];
        let ptr = cached.as_ptr();
        let len = cached.len();
        drop(buffers);

        // SAFETY: the slice points into a boxed buffer stored in `self.buffers`.
        // Box allocations remain stable when the Vec reallocates, and returned
        // span contents are tied to the lifetime of `&self`, so the cache
        // cannot be dropped while miette can still read the slice.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl SourceCode for RopeDiagnosticSource {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> std::result::Result<Box<dyn SpanContents<'a> + 'a>, MietteError> {
        let len = self.rope.len();
        let span_end = span
            .offset()
            .checked_add(span.len())
            .filter(|end| *end <= len)
            .ok_or(MietteError::OutOfBounds)?;
        if span.offset() > len {
            return Err(MietteError::OutOfBounds);
        }

        let line_type = LineType::LF;
        let start_line = self.rope.byte_to_line_idx(span.offset(), line_type);
        let end_byte = if span_end == span.offset() {
            span.offset()
        } else {
            span_end.saturating_sub(1)
        };
        let end_line = self.rope.byte_to_line_idx(end_byte, line_type);
        let context_start_line = start_line.saturating_sub(context_lines_before);
        let context_end_line = end_line
            .saturating_add(context_lines_after)
            .saturating_add(1)
            .min(self.rope.len_lines(line_type));
        let context_start = self.rope.line_to_byte_idx(context_start_line, line_type);
        let context_end = if context_end_line >= self.rope.len_lines(line_type) {
            len
        } else {
            self.rope.line_to_byte_idx(context_end_line, line_type)
        };
        let column = span
            .offset()
            .saturating_sub(self.rope.line_to_byte_idx(start_line, line_type));
        let line_count = context_end_line.saturating_sub(context_start_line).max(1);
        let data = self
            .rope
            .slice(context_start..context_end)
            .to_string()
            .into_bytes();
        let contents = MietteSpanContents::new_named(
            self.name.clone(),
            self.cache_bytes(data),
            (context_start, context_end.saturating_sub(context_start)).into(),
            context_start_line,
            column,
            line_count,
        )
        .with_language("dsql");
        Ok(Box::new(contents))
    }
}
