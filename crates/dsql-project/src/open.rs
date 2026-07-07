//! Bowl assembly: a loaded project becomes a settled language bowl.

use bowl::Bowl;

use dsql_core::catalog::insert_catalog;
use dsql_core::register_language;
use dsql_core::source::insert_source;

use super::config::{Project, Result};
use super::documents::load_project_documents;

/// Registers the language, installs the project's catalog, and inserts
/// every project document. Demand markers are the caller's choice — a CLI
/// check inserts `DiagnosticsDemand`, SQL generation `PlanDemand` +
/// `SqlDemand`, an LSP whatever its editor session needs.
pub async fn open_project_bowl(project: &Project) -> Result<Bowl> {
    let catalog = project.load_catalog()?;
    let documents = load_project_documents(project)?;

    let bowl = Bowl::new();
    register_language(&bowl).await;
    insert_catalog(&bowl, catalog).await;
    for document in documents {
        insert_source(&bowl, document.path.display().to_string(), &document.text).await;
    }
    Ok(bowl)
}
