//! Loads the imdb fixture project into a bowl and drives it.

use std::path::{Path, PathBuf};

use bowl::{Entity, Query, Singleton};
use dsql_core::facts::{Diagnostic, DiagnosticsDemand, PlanDemand, SqlDemand};
use dsql_core::sql::GeneratedSqlFact;
use dsql_project::{Project, find_root, open_project_bowl};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/imdb")
}

#[tokio::test]
async fn root_discovery_walks_upward() {
    let nested = fixture_dir().join("dsql/queries");
    assert_eq!(find_root(&nested).await, Some(fixture_dir().join("dsql")));
}

#[tokio::test]
async fn project_checks_clean_and_generates_sql() {
    {
        let project = Project::load_from(&fixture_dir())
            .await
            .expect("fixture project loads");
        assert_eq!(project.config.default_schema, "public");

        let bowl = open_project_bowl(&project).await.expect("bowl assembles");
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
            .await;

        let diagnostics = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(diagnostics, 0, "fixture project must check clean");

        let generated = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        let names: Vec<String> = generated
            .collect()
            .into_iter()
            .map(|(_, fact)| fact.0.output_name.clone())
            .collect();
        assert_eq!(names, vec!["title".to_string()]);
    }
}

#[tokio::test]
async fn scoped_project_resolves_imports_end_to_end() {
    {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/scoped");
        let project = Project::load_from(&fixture)
            .await
            .expect("scoped fixture loads");
        assert!(
            matches!(
                project.config.lint.unindexed_scan_severity,
                Some(dsql_project::LintSeverity::Warning)
            ),
            "the [lint] section parses"
        );

        let bowl = open_project_bowl(&project).await.expect("bowl assembles");
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
            .await;

        let diagnostics = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(
            diagnostics, 0,
            "the frontend scope must see shared fragments"
        );

        let generated = bowl
            .scoop::<Query<(Entity, &GeneratedSqlFact)>>()
            .await
            .len();
        assert_eq!(
            generated, 2,
            "plain and embedded queries must plan and render"
        );
    }
}

#[tokio::test]
async fn host_documents_load_whole() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/scoped");
    let project = Project::load_from(&fixture)
        .await
        .expect("scoped fixture loads");

    // The loader does not extract or classify: hosts arrive whole; the
    // source model routes them at insert and the bowl derives regions.
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("documents load");
    let hosts: Vec<_> = documents
        .iter()
        .filter(|document| document.path.extension().and_then(|ext| ext.to_str()) == Some("ts"))
        .collect();
    assert_eq!(hosts.len(), 1, "the fixture has one host source");
    let host = hosts[0];
    assert_eq!(host.scope, "frontend");
    assert!(host.text.contains("import"), "hosts carry their full text");
    assert!(host.text.contains("query TitlePanel"));
}

#[tokio::test]
async fn unknown_scope_imports_fail_project_loading() {
    let dir = std::env::temp_dir().join("dsql-unknown-import-fixture");
    let root = dir.join("dsql");
    std::fs::create_dir_all(&root).expect("fixture dir");
    std::fs::write(
        root.join("dsql.toml"),
        "database_url = \"x\"\n\n[resolution.frontend]\ndocuments = []\nimports = [\"missing\"]\n",
    )
    .expect("fixture config");

    let error = Project::load_from(&dir)
        .await
        .expect_err("unknown import must fail");
    assert!(
        error
            .to_string()
            .contains("imports unknown scope `missing`"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn documents_owned_by_two_scopes_fail_loading() {
    let dir = std::env::temp_dir().join("dsql-dup-ownership-fixture");
    let root = dir.join("dsql");
    // Document paths resolve from the project base, beside dsql/.
    let queries = dir.join("queries");
    std::fs::create_dir_all(&queries).expect("fixture dir");
    std::fs::write(
        root.join("dsql.toml"),
        "database_url = \"x\"\n\n[resolution.a]\ndocuments = [\"queries\"]\n\n[resolution.b]\ndocuments = [\"queries\"]\n",
    )
    .expect("fixture config");
    std::fs::write(
        queries.join("q.dsql"),
        "query Q {\n  title {\n    id\n  }\n}\n",
    )
    .expect("fixture doc");

    let project = Project::load_from(&dir).await.expect("config parses");
    let error = dsql_project::load_project_documents(&project)
        .await
        .expect_err("dual ownership must fail");
    assert!(
        error.to_string().contains("owned by both scope"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn schema_directory_round_trips_and_drops_stale_tables() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/imdb");
    let project = Project::load_from(&fixture)
        .await
        .expect("imdb fixture loads");
    let metadata = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("schema loads");

    let dir = std::env::temp_dir().join(format!("dsql-schema-roundtrip-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    dsql_project::store_metadata_dir(&metadata, &dir)
        .await
        .expect("schema stores");
    let reloaded = dsql_project::load_metadata_dir(&dir)
        .await
        .expect("stored schema loads");
    let mut canonical = metadata.clone();
    canonical.canonicalize();
    assert_eq!(reloaded, canonical);

    // A table file for a dropped table disappears on the next store.
    let stale = dir.join("public/dropped_table.yaml");
    std::fs::write(&stale, "stale").expect("stale file writes");
    dsql_project::store_metadata_dir(&metadata, &dir)
        .await
        .expect("schema restores");
    assert!(!stale.exists(), "stale table files are removed");

    std::fs::remove_dir_all(&dir).expect("cleanup");
}
