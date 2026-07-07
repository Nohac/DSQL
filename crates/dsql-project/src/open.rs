//! Bowl assembly: a loaded project becomes a settled language bowl.

use std::collections::BTreeMap;

use bowl::{Bowl, Singleton};

use dsql_core::catalog::insert_catalog;
use dsql_core::register_language;
use dsql_core::source::{ResolutionScope, ScopeImports, insert_source_scoped};

use super::config::{Project, Result};
use super::documents::load_project_documents;

/// Registers the language and populates a fresh bowl with the project's
/// contents. Demand markers are the caller's choice — a CLI check inserts
/// `DiagnosticsDemand`, SQL generation `PlanDemand` + `SqlDemand`, an LSP
/// whatever its editor session needs.
pub async fn open_project_bowl(project: &Project) -> Result<Bowl> {
    let bowl = Bowl::new();
    register_language(&bowl).await;
    populate_project_bowl(&bowl, project).await?;
    Ok(bowl)
}

/// Populates an already-registered bowl with the project's catalog, scope
/// configuration, and documents.
pub async fn populate_project_bowl(bowl: &Bowl, project: &Project) -> Result<()> {
    let catalog = project.load_catalog()?;
    let documents = load_project_documents(project)?;

    insert_catalog(bowl, catalog).await;

    let imports: BTreeMap<String, Vec<String>> = project
        .config
        .resolution
        .iter()
        .map(|(scope, config)| (scope.clone(), config.imports.clone()))
        .collect();
    bowl.insert((Singleton::<ScopeImports>::new(), ScopeImports(imports)))
        .await;

    for document in documents {
        insert_source_scoped(
            bowl,
            document.path.display().to_string(),
            &document.text,
            ResolutionScope(document.scope),
        )
        .await;
    }
    Ok(())
}
