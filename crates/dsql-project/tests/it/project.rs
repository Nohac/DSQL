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
async fn init_scaffolds_a_loadable_project() {
    let dir = std::env::temp_dir().join(format!("dsql-init-fixture-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    std::fs::create_dir_all(&dir).expect("fixture dir");

    let project = dsql_project::init_project(&dir, None)
        .await
        .expect("init scaffolds");
    assert!(project.schema.is_dir(), "schema/ directory exists");
    insta::assert_snapshot!(
        std::fs::read_to_string(project.root.join("dsql.toml")).expect("config written")
    );

    let reloaded = Project::load_from(&dir).await.expect("project round-trips");
    assert_eq!(reloaded.config.database_url, "<database url>");
    assert_eq!(
        reloaded.config.resolution["main"].documents,
        vec!["**/*.dsql".to_string()]
    );

    // A second init must not clobber the existing configuration.
    let error = dsql_project::init_project(&dir, Some("postgres://x".to_string()))
        .await
        .expect_err("re-init refuses");
    assert!(
        error.to_string().contains("already exists"),
        "unexpected error: {error}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[tokio::test]
async fn init_rolls_back_when_the_schema_directory_is_blocked() {
    let dir = std::env::temp_dir().join(format!("dsql-init-blocked-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    // A regular file where schema/ must go blocks initialization.
    std::fs::create_dir_all(dir.join("dsql")).expect("fixture dir");
    std::fs::write(dir.join("dsql/schema"), "blocker").expect("blocking file");

    let error = dsql_project::init_project(&dir, None)
        .await
        .expect_err("blocked schema fails init");
    assert!(
        error.to_string().contains("schema"),
        "the error names the failing path: {error}"
    );
    assert!(
        !dir.join("dsql/dsql.toml").exists(),
        "a failed init must not leave a config behind: {error}"
    );

    // Removing the blocker makes a retry succeed — nothing was stranded.
    std::fs::remove_file(dir.join("dsql/schema")).expect("unblock");
    dsql_project::init_project(&dir, None)
        .await
        .expect("retry succeeds after unblocking");

    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[tokio::test]
async fn init_escapes_hostile_database_urls() {
    let dir = std::env::temp_dir().join(format!("dsql-init-escape-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    std::fs::create_dir_all(&dir).expect("fixture dir");

    // Quotes and backslashes are representable TOML: they MUST round-trip.
    let quoted = "postgres://user:pa\"ss\\word@host/db".to_string();
    let project = dsql_project::init_project(&dir, Some(quoted.clone()))
        .await
        .expect("representable URLs init");
    assert_eq!(project.config.database_url, quoted);
    let reloaded = Project::load_from(&dir).await.expect("round-trips");
    assert_eq!(reloaded.config.database_url, quoted);
    std::fs::remove_dir_all(&dir).expect("reset");
    std::fs::create_dir_all(&dir).expect("fixture dir");

    // A newline may be unrepresentable in the starter: round-trip exactly
    // or fail cleanly — never write invalid TOML that strands the project.
    let hostile = "postgres://user:pa\"ss\\wo\nrd@host/db".to_string();
    match dsql_project::init_project(&dir, Some(hostile.clone())).await {
        Ok(project) => {
            assert_eq!(project.config.database_url, hostile);
            let reloaded = Project::load_from(&dir).await.expect("round-trips");
            assert_eq!(reloaded.config.database_url, hostile);
        }
        Err(error) => {
            assert!(
                !dir.join("dsql/dsql.toml").exists(),
                "a rejected URL must not leave a config behind: {error}"
            );
        }
    }

    std::fs::remove_dir_all(&dir).expect("cleanup");
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

/// Single-star globs must not cross directory boundaries (the manual
/// reserved-pruning walk keeps glob::glob's literal-separator behavior).
#[tokio::test]
async fn single_star_globs_stay_in_their_directory() {
    let dir = std::env::temp_dir().join(format!("dsql-glob-literal-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    std::fs::create_dir_all(dir.join("dsql")).expect("dirs");
    std::fs::create_dir_all(dir.join("queries/nested")).expect("dirs");
    std::fs::write(
        dir.join("dsql/dsql.toml"),
        "database_url = \"x\"\n\n[resolution.flat]\ndocuments = [\"queries/*.dsql\"]\n",
    )
    .expect("config");
    let doc = "query Q {\n  title(limit 1) {\n    id\n  }\n}\n";
    std::fs::write(dir.join("queries/top.dsql"), doc).expect("top doc");
    std::fs::write(dir.join("queries/nested/deep.dsql"), doc).expect("deep doc");

    let project = Project::load_from(&dir).await.expect("project loads");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("documents load");
    let paths: Vec<String> = documents
        .iter()
        .map(|document| document.path.display().to_string())
        .collect();
    assert!(
        paths.iter().any(|path| path.ends_with("top.dsql")),
        "the flat file matches, got {paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.ends_with("deep.dsql")),
        "a single star must not cross directories, got {paths:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
