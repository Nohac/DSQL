//! Bowl assembly: a loaded project becomes a settled language bowl.

use std::collections::BTreeMap;

use bowl::{Bowl, Singleton};

use dsql_core::catalog::insert_catalog;
use dsql_core::embedding::EmbeddedPattern;
use dsql_core::facts::Severity;
use dsql_core::lint::LintConfig;
use dsql_core::register_language;
use dsql_core::source::{ResolutionScope, ScopeImports, insert_host_source, insert_source_scoped};

use super::config::{LintSeverity, Project, Result};
use super::documents::DocumentKind;
use super::documents::load_project_documents;
use super::embedding::typescript_pattern;

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
        let path = document.path.display().to_string();
        let scope = ResolutionScope(document.scope);
        match document.kind {
            DocumentKind::Dsql => {
                insert_source_scoped(bowl, path, &document.text, scope).await;
            }
            DocumentKind::EmbeddingHost => {
                insert_host_source(bowl, path, &document.text, scope).await;
            }
        }
    }
    Ok(())
}
