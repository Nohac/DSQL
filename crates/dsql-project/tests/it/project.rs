//! Loads the imdb fixture project into a bowl and drives it.

use std::path::{Path, PathBuf};

use bowl::{Entity, Query};
use dsql_core::facts::{Diagnostic, arm_generate_demands};
use dsql_core::source::SourceKind;
use dsql_core::sql::GeneratedSqlFact;
use dsql_project::{Project, ProjectError, find_root, open_project_bowl};
use tempfile::TempDir;

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/it/fixture/{name}"))
}

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent directory");
    }
    std::fs::write(path, contents).expect("fixture file");
}

fn scratch() -> TempDir {
    tempfile::tempdir().expect("scratch directory")
}

fn scratch_project(config: &str) -> TempDir {
    let scratch = scratch();
    write_file(scratch.path(), "dsql/dsql.toml", config);
    scratch
}

#[tokio::test]
async fn root_discovery_walks_upward() {
    let fixture = fixture_dir("imdb");
    let nested = fixture.join("dsql/queries");
    assert_eq!(find_root(&nested).await, Some(fixture.join("dsql")));
}

#[tokio::test]
async fn project_checks_clean_and_generates_sql() {
    let project = Project::load_from(&fixture_dir("imdb"))
        .await
        .expect("fixture project loads");
    assert_eq!(project.config.default_schema, "public");

    let bowl = open_project_bowl(&project).await.expect("bowl assembles");
    arm_generate_demands(&bowl).await;

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

#[tokio::test]
async fn scoped_project_resolves_imports_end_to_end() {
    let project = Project::load_from(&fixture_dir("scoped"))
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
    arm_generate_demands(&bowl).await;

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

#[tokio::test]
async fn host_documents_load_whole() {
    let fixture = fixture_dir("scoped");
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
        .filter(|document| matches!(document.kind, SourceKind::Embedded(_)))
        .collect();
    assert_eq!(hosts.len(), 1, "the fixture has one host source");
    let host = hosts[0];
    assert_eq!(host.scope, "frontend");
    assert!(host.text.contains("import"), "hosts carry their full text");
    assert!(host.text.contains("query TitlePanel"));
}

#[tokio::test]
async fn configured_resolvers_drive_discovery_independently_of_extensions() {
    let scratch = scratch_project("database_url = \"x\"\n");
    let dir = scratch.path();
    write_file(dir, "dsql/plain.dsql", "query Plain { title { id } }\n");
    write_file(dir, "dsql/host.ts", "export const value = 1;\n");
    write_file(dir, "dsql/notes.md", "not a source\n");

    let project = Project::load_from(dir).await.expect("project loads");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("default documents load");
    let names: Vec<_> = documents
        .iter()
        .filter_map(|document| document.path.file_name().and_then(|name| name.to_str()))
        .collect();
    assert_eq!(
        names,
        vec!["plain.dsql"],
        "default discovery is standalone-only"
    );

    write_file(dir, "sources/plain.query", "query Plain { title { id } }\n");
    write_file(
        dir,
        "sources/host.component",
        "export const value = dsql`query Host { title { id } }`;\n",
    );
    write_file(dir, "sources/notes.md", "not a source\n");
    write_file(
        dir,
        "dsql/dsql.toml",
        "database_url = \"x\"\ndocuments = [\n  { resolver = \"dsql\", paths = [\"sources/plain.query\"] },\n  { resolver = \"typescript\", paths = [\"sources/host.component\"] },\n]\n",
    );

    let project = Project::load_from(dir)
        .await
        .expect("configured project loads");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("configured documents load");
    let classified: Vec<_> = documents
        .iter()
        .filter_map(|document| Some((document.path.file_name()?.to_str()?, document.kind.clone())))
        .collect();
    assert_eq!(
        classified,
        vec![
            (
                "host.component",
                SourceKind::Embedded("typescript".to_string())
            ),
            ("plain.query", SourceKind::Dsql),
        ],
        "configured discovery accepts exactly the shared source kinds"
    );

    // Named scopes replace the top-level default routing everywhere. An
    // output may therefore overlap the ignored group, while the effective
    // scope remains resolver-driven regardless of its file extension.
    write_file(
        dir,
        "dsql/dsql.toml",
        "database_url = \"x\"\ndocuments = [{ resolver = \"typescript\", paths = [\"ignored\"] }]\n\n[resolution.frontend]\ndocuments = [{ resolver = \"dsql\", paths = [\"sources/plain.query\"] }]\n\n[generate.typescript]\noutputs = [\"ignored\"]\n",
    );

    let project = Project::load_from(dir)
        .await
        .expect("ignored default routes do not reserve generator outputs");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("named documents load");
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].scope, "frontend");
    assert_eq!(documents[0].kind, SourceKind::Dsql);
    assert_eq!(
        documents[0].path.file_name().and_then(|name| name.to_str()),
        Some("plain.query")
    );
}

#[tokio::test]
async fn invalid_scope_import_graphs_are_rejected() {
    let cases = [
        (
            "unknown import",
            "database_url = \"x\"\n\n[resolution.frontend]\ndocuments = []\nimports = [\"missing\"]\n",
            "scope `frontend` imports unknown scope `missing`",
        ),
        (
            "cyclic imports",
            indoc::indoc! {r#"
                database_url = "x"

                [resolution.a]
                documents = []
                imports = ["b"]

                [resolution.b]
                documents = []
                imports = ["c"]

                [resolution.c]
                documents = []
                imports = ["a"]
            "#},
            "cyclic scope import: a -> b -> c -> a",
        ),
    ];
    for (case, config, expected) in cases {
        let scratch = scratch_project(config);
        let error = Project::load_from(scratch.path())
            .await
            .expect_err("invalid imports must fail");
        assert_eq!(error.to_string(), expected, "{case}");
    }
}

#[tokio::test]
async fn duplicate_document_assignments_report_both_owners() {
    let cases = [
        (
            "two scopes",
            "database_url = \"x\"\n\n[resolution.a]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/source.any\"] }]\n\n[resolution.b]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/source.any\"] }]\n",
            ("a", "dsql", "b", "dsql"),
        ),
        (
            "two resolvers",
            "database_url = \"x\"\n\n[resolution.frontend]\ndocuments = [\n  { resolver = \"dsql\", paths = [\"queries/source.any\"] },\n  { resolver = \"typescript\", paths = [\"queries/source.any\"] },\n]\n",
            ("frontend", "dsql", "frontend", "typescript"),
        ),
    ];
    for (case, config, expected) in cases {
        let scratch = scratch_project(config);
        write_file(
            scratch.path(),
            "queries/source.any",
            "query Q { title { id } }\n",
        );
        let project = Project::load_from(scratch.path())
            .await
            .expect("config parses");
        let error = dsql_project::load_project_documents(&project)
            .await
            .expect_err("duplicate assignment must fail");
        let actual = if let ProjectError::DuplicateDocumentAssignment {
            path,
            first_scope,
            first_resolver,
            second_scope,
            second_resolver,
        } = error
        {
            Some((
                path,
                first_scope,
                first_resolver,
                second_scope,
                second_resolver,
            ))
        } else {
            None
        };
        assert_eq!(
            actual,
            Some((
                scratch.path().join("queries/source.any"),
                expected.0.to_string(),
                expected.1.to_string(),
                expected.2.to_string(),
                expected.3.to_string(),
            )),
            "{case}"
        );
    }
}

#[tokio::test]
async fn init_scaffolds_a_loadable_project() {
    let scratch = scratch();
    let dir = scratch.path();

    let project = dsql_project::init_project(dir, None)
        .await
        .expect("init scaffolds");
    assert!(project.schema.is_dir(), "schema/ directory exists");
    insta::assert_snapshot!(
        std::fs::read_to_string(project.root.join("dsql.toml")).expect("config written")
    );

    let reloaded = Project::load_from(dir).await.expect("project round-trips");
    assert_eq!(reloaded.config.database_url, "<database url>");
    assert_eq!(
        reloaded.config.resolution["main"].documents,
        vec![dsql_project::DocumentConfig {
            resolver: "dsql".to_string(),
            paths: vec!["**/*.dsql".to_string()],
        }]
    );

    // A second init must not clobber the existing configuration.
    let error = dsql_project::init_project(dir, Some("postgres://x".to_string()))
        .await
        .expect_err("re-init refuses");
    assert!(
        error.to_string().contains("already exists"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn init_rolls_back_when_the_schema_directory_is_blocked() {
    let scratch = scratch();
    let dir = scratch.path();
    // A regular file where schema/ must go blocks initialization.
    write_file(dir, "dsql/schema", "blocker");

    let error = dsql_project::init_project(dir, None)
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
    dsql_project::init_project(dir, None)
        .await
        .expect("retry succeeds after unblocking");
}

#[tokio::test]
async fn init_escapes_hostile_database_urls() {
    // Quotes and backslashes are representable TOML: they MUST round-trip.
    let quoted_scratch = scratch();
    let dir = quoted_scratch.path();
    let quoted = "postgres://user:pa\"ss\\word@host/db".to_string();
    let project = dsql_project::init_project(dir, Some(quoted.clone()))
        .await
        .expect("representable URLs init");
    assert_eq!(project.config.database_url, quoted);
    let reloaded = Project::load_from(dir).await.expect("round-trips");
    assert_eq!(reloaded.config.database_url, quoted);

    // A newline may be unrepresentable in the starter: round-trip exactly
    // or fail cleanly — never write invalid TOML that strands the project.
    let hostile_scratch = scratch();
    let dir = hostile_scratch.path();
    let hostile = "postgres://user:pa\"ss\\wo\nrd@host/db".to_string();
    match dsql_project::init_project(dir, Some(hostile.clone())).await {
        Ok(project) => {
            assert_eq!(project.config.database_url, hostile);
            let reloaded = Project::load_from(dir).await.expect("round-trips");
            assert_eq!(reloaded.config.database_url, hostile);
        }
        Err(error) => {
            assert!(
                !dir.join("dsql/dsql.toml").exists(),
                "a rejected URL must not leave a config behind: {error}"
            );
        }
    }
}

#[tokio::test]
async fn schema_directory_round_trips_and_drops_stale_tables() {
    let fixture = fixture_dir("imdb");
    let project = Project::load_from(&fixture)
        .await
        .expect("imdb fixture loads");
    let metadata = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("schema loads");

    let scratch = scratch();
    let dir = scratch.path();
    dsql_project::store_metadata_dir(&metadata, dir)
        .await
        .expect("schema stores");
    let reloaded = dsql_project::load_metadata_dir(dir)
        .await
        .expect("stored schema loads");
    let mut canonical = metadata.clone();
    canonical.canonicalize();
    assert_eq!(reloaded, canonical);

    // A table file for a dropped table disappears on the next store.
    let stale = dir.join("public/dropped_table.yaml");
    write_file(dir, "public/dropped_table.yaml", "stale");
    dsql_project::store_metadata_dir(&metadata, dir)
        .await
        .expect("schema restores");
    assert!(!stale.exists(), "stale table files are removed");
}

/// Single-star globs must not cross directory boundaries (the manual
/// reserved-pruning walk keeps glob::glob's literal-separator behavior).
#[tokio::test]
async fn single_star_globs_stay_in_their_directory() {
    let scratch = scratch_project(
        "database_url = \"x\"\n\n[resolution.flat]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/*.dsql\"] }]\n",
    );
    let dir = scratch.path();
    let doc = "query Q {\n  title(limit 1) {\n    id\n  }\n}\n";
    write_file(dir, "queries/top.dsql", doc);
    write_file(dir, "queries/nested/deep.dsql", doc);

    let project = Project::load_from(dir).await.expect("project loads");
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
}
