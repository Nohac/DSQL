use dsql_core::{
    Catalog, Diagnostic, PostgresSqlOptions, Severity, SourceSnapshot, check_file_with_catalog,
    format_file, generate_postgres_sql_with_options, lint_file_with_catalog, parse_source,
    plan_file_with_catalog,
};
use insta::Settings;
use sqlx::{Row, postgres::PgPoolOptions};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[test]
fn valid_query_fixtures_compile_against_imdb_schema() {
    let catalog = imdb_catalog();
    let fixtures = dsql_fixtures("queries/valid");
    assert!(!fixtures.is_empty(), "expected valid query fixtures");

    for fixture in fixtures {
        let source = fs::read_to_string(&fixture).unwrap();
        let parsed = parse_source(SourceSnapshot::from_string(source));
        assert_no_diagnostics(&fixture, "parse", &parsed.diagnostics);

        let checked = check_file_with_catalog(&parsed.source_file, &catalog);
        assert!(
            checked.errors.is_empty(),
            "{} check errors: {:?}",
            fixture.display(),
            checked.errors
        );
        assert_no_error_diagnostics(&fixture, "check", &checked.diagnostics);

        let linted = lint_file_with_catalog(&parsed.source_file, &catalog);
        assert_no_error_diagnostics(&fixture, "lint", &linted.diagnostics);

        let formatted = format_file(&parsed);
        assert_no_error_diagnostics(&fixture, "format", &formatted.diagnostics);

        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert_no_error_diagnostics(&fixture, "plan", &planned.diagnostics);
        assert!(
            !planned.queries.is_empty(),
            "{} should produce at least one query plan",
            fixture.display()
        );

        let mut sql = String::new();
        for query in &planned.queries {
            let generated = generate_postgres_sql_with_options(
                query,
                &catalog,
                PostgresSqlOptions {
                    collection_limit: Some(10),
                },
            )
            .unwrap_or_else(|error| panic!("{} SQL generation failed: {error}", fixture.display()));
            sql.push_str("-- ");
            sql.push_str(&generated.output_name);
            sql.push('\n');
            sql.push_str(generated.sql.trim_end());
            sql.push('\n');
        }

        snapshot_fixture(&fixture, "sql", sql.trim_end());
    }
}

#[test]
fn invalid_query_fixtures_report_diagnostics_against_imdb_schema() {
    let catalog = imdb_catalog();
    let fixtures = dsql_fixtures("queries/invalid");
    assert!(!fixtures.is_empty(), "expected invalid query fixtures");

    for fixture in fixtures {
        let source = fs::read_to_string(&fixture).unwrap();
        let parsed = parse_source(SourceSnapshot::from_string(source));
        let checked = check_file_with_catalog(&parsed.source_file, &catalog);
        let diagnostics = parsed
            .diagnostics
            .iter()
            .chain(checked.diagnostics.iter())
            .collect::<Vec<_>>();

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error),
            "{} should produce at least one error diagnostic",
            fixture.display()
        );

        snapshot_fixture(&fixture, "diagnostics", &format_diagnostics(&diagnostics));
    }
}

#[tokio::test]
async fn valid_query_fixtures_execute_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };

    let catalog = imdb_catalog();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();

    for fixture in dsql_fixtures("queries/valid") {
        let source = fs::read_to_string(&fixture).unwrap();
        let parsed = parse_source(SourceSnapshot::from_string(source));
        assert_no_diagnostics(&fixture, "parse", &parsed.diagnostics);

        let checked = check_file_with_catalog(&parsed.source_file, &catalog);
        assert!(
            checked.errors.is_empty(),
            "{} check errors: {:?}",
            fixture.display(),
            checked.errors
        );

        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert_no_error_diagnostics(&fixture, "plan", &planned.diagnostics);

        for query in &planned.queries {
            let generated = generate_postgres_sql_with_options(
                query,
                &catalog,
                PostgresSqlOptions {
                    collection_limit: Some(10),
                },
            )
            .unwrap();
            let row = sqlx::query(&generated.sql).fetch_one(&pool).await.unwrap();
            let value: serde_json::Value = row.try_get(0).unwrap();
            assert!(
                value.is_array() || value.is_object(),
                "{} generated JSON should be an array or object",
                fixture.display()
            );
        }
    }
}

#[tokio::test]
async fn integration_query_fixtures_match_expected_output_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };

    let catalog = imdb_catalog();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();

    for fixture in dsql_fixtures("queries/integration") {
        let source = fs::read_to_string(&fixture).unwrap();
        let parsed = parse_source(SourceSnapshot::from_string(source));
        assert_no_diagnostics(&fixture, "parse", &parsed.diagnostics);

        let checked = check_file_with_catalog(&parsed.source_file, &catalog);
        assert!(
            checked.errors.is_empty(),
            "{} check errors: {:?}",
            fixture.display(),
            checked.errors
        );

        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert_no_error_diagnostics(&fixture, "plan", &planned.diagnostics);
        assert_eq!(
            planned.queries.len(),
            1,
            "{} should contain exactly one integration query",
            fixture.display()
        );

        let generated = generate_postgres_sql_with_options(
            &planned.queries[0],
            &catalog,
            PostgresSqlOptions {
                collection_limit: Some(10),
            },
        )
        .unwrap();
        let row = sqlx::query(&generated.sql).fetch_one(&pool).await.unwrap();
        let value: serde_json::Value = row.try_get(0).unwrap();
        let output = serde_json::to_string_pretty(&value).unwrap();
        snapshot_fixture(&fixture, "output", &output);
    }
}

fn imdb_catalog() -> Catalog {
    dsql_project::load_metadata_dir(&fixture_root().join("schema/imdb"))
        .unwrap()
        .into_catalog()
        .unwrap()
        .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

fn dsql_fixtures(relative_dir: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_dsql_files(&fixture_root().join(relative_dir), &mut files);
    files.sort();
    files
}

fn collect_dsql_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_dsql_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path);
        }
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn snapshot_fixture(fixture: &Path, phase: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(fixture_root().join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!(snapshot_name(fixture, phase), contents);
    });
}

fn snapshot_name(fixture: &Path, phase: &str) -> String {
    let stem = fixture
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("fixture")
        .replace('-', "_");
    format!("fixtures__{stem}_{phase}")
}

fn format_diagnostics(diagnostics: &[&Diagnostic]) -> String {
    let mut output = String::new();
    for diagnostic in diagnostics {
        output.push_str(&format!(
            "{:?} {:?} {:?} {}..{}: {}\n",
            diagnostic.source,
            diagnostic.severity,
            diagnostic.code,
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.message
        ));
    }
    output
}

fn assert_no_diagnostics(fixture: &Path, phase: &str, diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "{} {phase} diagnostics: {diagnostics:?}",
        fixture.display()
    );
}

fn assert_no_error_diagnostics(fixture: &Path, phase: &str, diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error),
        "{} {phase} diagnostics: {diagnostics:?}",
        fixture.display()
    );
}
