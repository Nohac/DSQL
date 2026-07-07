//! Loads the imdb fixture project into a bowl and drives it.

use std::path::{Path, PathBuf};

use bowl::{Entity, Query, Singleton};
use dsql_core::facts::{Diagnostic, DiagnosticsDemand, PlanDemand, SqlDemand};
use dsql_core::sql::GeneratedSqlFact;
use dsql_project::{Project, find_root, open_project_bowl};
use futures::executor::block_on;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/imdb")
}

#[test]
fn root_discovery_walks_upward() {
    let nested = fixture_dir().join("dsql/queries");
    assert_eq!(find_root(&nested), Some(fixture_dir().join("dsql")));
}

#[test]
fn project_checks_clean_and_generates_sql() {
    block_on(async {
        let project = Project::load_from(&fixture_dir()).expect("fixture project loads");
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
    });
}

#[test]
fn scoped_project_resolves_imports_end_to_end() {
    block_on(async {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/scoped");
        let project = Project::load_from(&fixture).expect("scoped fixture loads");

        let bowl = open_project_bowl(&project).await.expect("bowl assembles");
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
            .await;

        let diagnostics = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(diagnostics, 0, "the frontend scope must see shared fragments");

        let generated = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await.len();
        assert_eq!(generated, 1, "the imported fragment must plan and render");
    });
}

#[test]
fn unknown_scope_imports_fail_project_loading() {
    let dir = std::env::temp_dir().join("dsql-unknown-import-fixture");
    let root = dir.join("dsql");
    std::fs::create_dir_all(&root).expect("fixture dir");
    std::fs::write(
        root.join("dsql.toml"),
        "database_url = \"x\"\n\n[resolution.frontend]\ndocuments = []\nimports = [\"missing\"]\n",
    )
    .expect("fixture config");

    let error = Project::load_from(&dir).expect_err("unknown import must fail");
    assert!(
        error.to_string().contains("imports unknown scope `missing`"),
        "unexpected error: {error}"
    );
}

#[test]
fn documents_owned_by_two_scopes_fail_loading() {
    let dir = std::env::temp_dir().join("dsql-dup-ownership-fixture");
    let root = dir.join("dsql");
    let queries = root.join("queries");
    std::fs::create_dir_all(&queries).expect("fixture dir");
    std::fs::write(
        root.join("dsql.toml"),
        "database_url = \"x\"\n\n[resolution.a]\ndocuments = [\"queries\"]\n\n[resolution.b]\ndocuments = [\"queries\"]\n",
    )
    .expect("fixture config");
    std::fs::write(queries.join("q.dsql"), "query Q {\n  title {\n    id\n  }\n}\n")
        .expect("fixture doc");

    let project = Project::load_from(&dir).expect("config parses");
    let error =
        dsql_project::load_project_documents(&project).expect_err("dual ownership must fail");
    assert!(
        error.to_string().contains("owned by both scope"),
        "unexpected error: {error}"
    );
}
