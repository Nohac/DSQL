//! Command implementations over the project bowl.

use bowl::{Entity, Query, Singleton};
use futures::executor::block_on;

use dsql_core::facts::{DiagnosticsDemand, PlanDemand, SqlDemand};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_project::{Project, ProjectError, load_project_documents, open_project_bowl};

type Outcome = Result<bool, ProjectError>;

/// Prints every diagnostic in the project as a miette report with its
/// source excerpt, sorted by file and span. Returns true when the project
/// is clean.
pub fn check() -> Outcome {
    let project = Project::load()?;
    block_on(async {
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
    })
}

/// Prints the generated PostgreSQL for every query plan in the project.
pub fn sql(collection_limit: Option<u64>) -> Outcome {
    let project = Project::load()?;
    block_on(async {
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
    })
}

/// Formats project documents in place; with `check`, reports instead.
/// Returns true when nothing needed changing.
pub fn fmt(check_only: bool) -> Outcome {
    let project = Project::load()?;
    let documents = load_project_documents(&project)?;

    let mut clean = true;
    for document in documents {
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
            std::fs::write(&document.path, formatted.text).map_err(|source| {
                ProjectError::Read {
                    path: document.path.clone(),
                    source,
                }
            })?;
            println!("{}: reformatted", document.path.display());
        }
    }
    Ok(clean)
}

/// Writes the artifact tree and runs the configured host generator.
pub fn generate(collection_limit: Option<u64>) -> Outcome {
    let project = Project::load()?;
    let output = dsql_generate::generate_project(
        &project,
        dsql_generate::GenerateOptions { collection_limit },
    )
    .map_err(|error| {
        eprintln!("{error}");
        ProjectError::MissingRoot(project.root.clone())
    });
    match output {
        Ok(output) => {
            for path in &output.written {
                println!("{}: written", path.display());
            }
            println!("manifest: {}", output.manifest_path.display());
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Introspects the configured database and writes the schema directory.
pub fn introspect() -> Outcome {
    let project = Project::load()?;
    let runtime = tokio::runtime::Runtime::new().map_err(|source| ProjectError::Read {
        path: project.root.clone(),
        source,
    })?;
    let metadata = runtime.block_on(dsql_introspection::introspect_postgres(
        &project.config.database_url,
    ));
    match metadata {
        Ok(metadata) => {
            dsql_project::store_metadata_dir(&metadata, &project.schema)?;
            println!("schema written to {}", project.schema.display());
            Ok(true)
        }
        Err(error) => {
            eprintln!("introspection failed: {error}");
            Ok(false)
        }
    }
}
