//! Storage-independent workspace population.

use std::collections::BTreeMap;

use bowl::{Entity, Query};
use dsql_core::catalog::Catalog;
use dsql_core::embedding::ExtractionRegistry;
use dsql_core::facts::{Severity, arm_generate_demands};
use dsql_core::input::{LanguageDocument, LanguageInputs, populate_language_bowl};
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::{
    ResolutionScope, ScopeDocument, ScopeDocuments, ScopeImports, ScopeOwnership, SourceKind,
};
use dsql_core::sql::GeneratedSqlFact;

use crate::render_diagnostic_facts;

#[tokio::test]
async fn normalized_inputs_drive_scopes_extraction_catalog_and_generation() {
    let bowl = language_bowl().await;
    arm_generate_demands(&bowl).await;
    let scope_documents = ScopeDocuments(vec![
        (
            "main".to_string(),
            vec![ScopeDocument {
                kind: SourceKind::Embedded("typescript".to_string()),
                paths: vec!["src/**/*.ts".to_string()],
            }],
        ),
        (
            "shared".to_string(),
            vec![ScopeDocument {
                kind: SourceKind::Dsql,
                paths: vec!["queries/**/*.dsql".to_string()],
            }],
        ),
    ]);
    populate_language_bowl(
        &bowl,
        LanguageInputs {
            catalog: Catalog::hardcoded(),
            documents: vec![
                LanguageDocument {
                    path: "queries/shared.dsql".to_string(),
                    text: "fragment SharedName on public::users { name }\n".to_string(),
                    scope: ResolutionScope("shared".to_string()),
                    kind: SourceKind::Dsql,
                },
                LanguageDocument {
                    path: "src/query.ts".to_string(),
                    text: "export const query = dsql`query Browser { public::users { ...SharedName } }`;"
                        .to_string(),
                    scope: ResolutionScope("main".to_string()),
                    kind: SourceKind::Embedded("typescript".to_string()),
                },
            ],
            scope_imports: ScopeImports(BTreeMap::from([(
                "main".to_string(),
                vec!["shared".to_string()],
            )])),
            scope_documents,
            extraction_registry: ExtractionRegistry::default(),
            lint: Some(LintConfig {
                unindexed_scan_severity: Some(Severity::Warning),
            }),
        },
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    let sql = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    assert_eq!(sql.collect().len(), 1);
    let scope_document_rows = bowl.scoop::<Query<(Entity, &ScopeDocuments)>>().await;
    let installed_scope_documents = scope_document_rows.collect();
    assert!(matches!(
        installed_scope_documents[0].1.ownership_of("src/query.ts"),
        ScopeOwnership::Unique(ref assignment)
            if assignment.scope == "main"
                && assignment.kind == SourceKind::Embedded("typescript".to_string())
    ));
    let lint_rows = bowl.scoop::<Query<(Entity, &LintConfig)>>().await;
    let lint = lint_rows.collect();
    assert_eq!(lint[0].1.unindexed_scan_severity, Some(Severity::Warning));
    let import_rows = bowl.scoop::<Query<(Entity, &ScopeImports)>>().await;
    let imports = import_rows.collect();
    assert_eq!(
        imports[0].1.visible_from("main").collect::<Vec<_>>(),
        vec!["main", "shared"]
    );
}
