//! Command implementations over the project bowl.

use bowl::{Entity, Query, Singleton};
use futures::executor::block_on;

use dsql_core::facts::{
    BelongsToFile, Diagnostic, DiagnosticsDemand, PlanDemand, Severity, Span, SqlDemand,
};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::source::FilePath;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use dsql_project::{Project, ProjectError, load_project_documents, open_project_bowl};

type Outcome = Result<bool, ProjectError>;

/// Prints every diagnostic in the project, sorted by file and span.
/// Returns true when the project is clean.
pub fn check() -> Outcome {
    let project = Project::load()?;
    block_on(async {
        let bowl = open_project_bowl(&project).await?;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        let rows = bowl
            .scoop::<Query<(Entity, &Severity, &Span, &Diagnostic, &BelongsToFile)>>()
            .await;
        let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
        let paths = paths.collect();

        let mut lines: Vec<String> = rows
            .collect()
            .into_iter()
            .map(|(_, severity, span, diagnostic, file)| {
                let path = paths
                    .iter()
                    .find(|(entity, _)| *entity == file.0)
                    .map(|(_, path)| path.0.as_str())
                    .unwrap_or("<unknown>");
                format!(
                    "{path}:{}..{}: {severity:?}: {}",
                    span.start, span.end, diagnostic.0
                )
            })
            .collect();
        lines.sort();

        for line in &lines {
            println!("{line}");
        }
        if lines.is_empty() {
            println!("no diagnostics");
        }
        Ok(lines.is_empty())
    })
}

/// Prints the generated PostgreSQL for every query plan in the project.
pub fn sql(collection_limit: Option<u64>) -> Outcome {
    let project = Project::load()?;
    block_on(async {
        let bowl = open_project_bowl(&project).await?;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
            .await;
        if collection_limit.is_some() {
            bowl.insert((Singleton::<SqlOptions>::new(), SqlOptions { collection_limit }))
                .await;
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
            eprintln!(
                "{}: skipped (parse errors)",
                document.path.display()
            );
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
