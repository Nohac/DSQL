//! Bowl assembly: a loaded project becomes a settled language bowl.

use std::collections::BTreeMap;

use bowl::{Bowl, Singleton};

use dsql_core::catalog::insert_catalog;
use dsql_core::embedding::EmbeddedPattern;
use dsql_core::facts::Severity;
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::{ResolutionScope, ScopeImports, insert_source_scoped};

use super::config::{LintSeverity, Project, Result};
use super::documents::load_project_documents;
use super::embedding::typescript_pattern;

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

/// Populates an already-registered bowl with the project's catalog, scope
/// configuration, and documents.
pub async fn populate_project_bowl(bowl: &Bowl, project: &Project) -> Result<()> {
    let catalog = project.load_catalog().await?;
    let documents = load_project_documents(project).await?;
    let pattern = typescript_pattern(project)?;

    insert_catalog(bowl, catalog).await;

    let imports: BTreeMap<String, Vec<String>> = project
        .config
        .resolution
        .iter()
        .map(|(scope, config)| (scope.clone(), config.imports.clone()))
        .collect();
    bowl.insert((Singleton::<ScopeImports>::new(), ScopeImports(imports)))
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
    bowl.insert((
        Singleton::<EmbeddedPattern>::new(),
        EmbeddedPattern(pattern),
    ))
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
