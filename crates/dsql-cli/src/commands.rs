//! Command implementations over the project bowl. All async: the binary
//! owns the runtime, libraries and commands only do async work.

use bowl::{Entity, Query, Singleton};

use dsql_core::facts::{DiagnosticsDemand, PlanDemand, SqlDemand};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_project::{Project, ProjectError, load_project_documents, open_project_bowl};

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
}

pub(crate) type Outcome = Result<bool, CliError>;

/// Prints every diagnostic in the project as a miette report with its
/// source excerpt, sorted by file and span. Returns true when the project
/// is clean.
pub async fn check() -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = open_project_bowl(&project).await?;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        let diagnostics = crate::render::collect_diagnostics(&bowl).await;
        for diagnostic in &diagnostics {
            print!("{}", crate::render::render(diagnostic));
        }
        if diagnostics.is_empty() {
            println!("no diagnostics");
        }
        Ok(diagnostics.is_empty())
    }
}

/// Prints the generated PostgreSQL for every query plan in the project.
pub async fn sql(collection_limit: Option<u64>) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = open_project_bowl(&project).await?;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
            .await;
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

/// Formats project documents in place; with `check`, reports instead.
/// Returns true when nothing needed changing.
pub async fn fmt(check_only: bool) -> Outcome {
    let project = Project::load().await?;
    fmt_project(&project, check_only).await
}

/// [`fmt`] against an explicit project root.
pub async fn fmt_project(project: &Project, check_only: bool) -> Outcome {
    let documents = load_project_documents(project).await?;

    let mut clean = true;
    for document in documents {
        // Host sources carry embedded regions; whole-file formatting is a
        // dsql-document affair until region-granular edits are supported.
        if document.is_embedding_host() {
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
pub async fn generate(collection_limit: Option<u64>) -> Outcome {
    let project = Project::load().await?;
    let output = dsql_generate::generate_project(
        &project,
        dsql_generate::GenerateOptions { collection_limit },
    )
    .await?;
    for path in &output.written {
        println!("{}: written", path.display());
    }
    println!("manifest: {}", output.manifest_path.display());
    Ok(true)
}

/// Introspects the configured database and writes the schema directory.
pub async fn introspect() -> Outcome {
    let project = Project::load().await?;
    let metadata = dsql_introspection::introspect_postgres(&project.config.database_url).await?;
    dsql_project::store_metadata_dir(&metadata, &project.schema).await?;
    println!("schema written to {}", project.schema.display());
    Ok(true)
}
