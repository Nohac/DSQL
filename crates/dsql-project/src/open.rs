//! Bowl assembly: a loaded project becomes a settled language bowl.

use bowl::{Bowl, Singleton};

use dsql_core::catalog::{CatalogSourceRoot, insert_catalog};
use dsql_core::embedding::ExtractionRegistry;
use dsql_core::facts::Severity;
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::{ResolutionScope, ScopeDocuments, ScopeImports, insert_source_scoped};

use super::config::{LintSeverity, Project, Result};
use super::embedding::extraction_registry;

/// Registers the language and populates a fresh bowl with the project's
/// contents. Demand markers are the caller's choice, armed through the
/// bundles in `dsql_core::facts` (`arm_generate_demands`,
/// `arm_editor_demands`) so no adapter hand-assembles an incomplete
/// pipeline.
pub async fn open_project_bowl(project: &Project) -> Result<Bowl> {
    let bowl = language_bowl().await;
    populate_project_bowl(&bowl, project).await?;
    Ok(bowl)
}

/// [`open_project_bowl`] with batch-analysis residency armed: source
/// ropes are evicted once parsed. For one-shot commands (check, sql,
/// validate, generate) — anything that keeps answering from the bowl
/// (debug sessions, daemons, editors) opens a resident bowl instead.
pub async fn open_analysis_bowl(project: &Project) -> Result<Bowl> {
    let bowl = language_bowl().await;
    dsql_core::source::arm_analysis_residency(&bowl).await;
    populate_project_bowl(&bowl, project).await?;
    Ok(bowl)
}

/// Populates an already-registered bowl with the project's catalog, scope
/// configuration, and documents.
pub async fn populate_project_bowl(bowl: &Bowl, project: &Project) -> Result<()> {
    populate_project_bowl_excluding(bowl, project, &[]).await
}

/// [`populate_project_bowl`] with consumer-declared reserved roots
/// excluded from document discovery (docs/spec/build-daemon.md).
pub async fn populate_project_bowl_excluding(
    bowl: &Bowl,
    project: &Project,
    extra_reserved: &[String],
) -> Result<()> {
    let catalog = project.load_catalog().await?;
    let documents =
        crate::documents::load_project_documents_excluding(project, extra_reserved).await?;
    let extraction_registry = extraction_registry(project)?;

    insert_catalog(bowl, catalog).await;
    bowl.insert((
        Singleton::<CatalogSourceRoot>::new(),
        CatalogSourceRoot(project.schema.clone()),
    ))
    .await;

    let imports = project.config.scope_imports();
    bowl.insert((Singleton::<ScopeImports>::new(), imports))
        .await;
    let scope_documents = crate::documents::scope_document_assignments(project);
    bowl.insert((
        Singleton::<ScopeDocuments>::new(),
        ScopeDocuments(scope_documents),
    ))
    .await;

    let lint = match project.config.lint.unindexed_scan_severity {
        None => LintConfig::default(),
        Some(LintSeverity::Off) => LintConfig {
            unindexed_scan_severity: None,
        },
        Some(LintSeverity::Info) => LintConfig {
            unindexed_scan_severity: Some(Severity::Info),
        },
        Some(LintSeverity::Warning) => LintConfig {
            unindexed_scan_severity: Some(Severity::Warning),
        },
        Some(LintSeverity::Error) => LintConfig {
            unindexed_scan_severity: Some(Severity::Error),
        },
    };
    bowl.insert((Singleton::<LintConfig>::new(), lint)).await;
    bowl.insert((Singleton::<ExtractionRegistry>::new(), extraction_registry))
        .await;

    for document in documents {
        insert_source_scoped(
            bowl,
            document.path.display().to_string(),
            &document.text,
            ResolutionScope(document.scope),
            document.kind,
        )
        .await;
    }
    Ok(())
}
