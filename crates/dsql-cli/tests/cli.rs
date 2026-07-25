//! End-to-end CLI tests: the built `dsql` binary runs against fixture
//! projects, pinning flag parsing, stdout, and exit codes — the surface
//! bowl-level tests cannot see.

use std::path::{Path, PathBuf};
use std::process::Output;

use facet_value::{Value, value};

fn dsql(dir: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_dsql"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("dsql runs")
}

fn dsql_with_database(dir: &Path, database_url: &str, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_dsql"))
        .args(args)
        .env("DSQL_DATABASE_URL", database_url)
        .current_dir(dir)
        .output()
        .expect("dsql runs")
}

fn execute_observatory(
    dir: &Path,
    database_url: &str,
    scope: &str,
    name: &str,
    bindings: &[&str],
) -> String {
    let mut args = vec!["operation", "execute", name, "--scope", scope];
    args.extend_from_slice(bindings);
    let output = dsql_with_database(dir, database_url, &args);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    stdout(&output)
}

fn output_json(output: &str) -> Value {
    facet_json::from_str(output).expect("operation output is JSON")
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .as_object()
        .and_then(|object| object.get(name))
        .expect("JSON object has expected field")
}

fn integer(value: &Value) -> i64 {
    value
        .as_number()
        .and_then(|number| number.to_i64())
        .expect("JSON value is an integer")
}

fn string(value: &Value) -> &str {
    value
        .as_string()
        .map(|value| value.as_str())
        .expect("JSON value is a string")
}

fn observatory_reading_ids(output: &str) -> Vec<i64> {
    let output = output_json(output);
    field(field(&output, "sensors"), "readings")
        .as_array()
        .expect("sensor reading window")
        .iter()
        .map(|reading| integer(field(reading, "id")))
        .collect()
}

fn observatory_root_reading_ids(output: &str) -> Vec<i64> {
    let output = output_json(output);
    field(&output, "readings")
        .as_array()
        .expect("root reading collection")
        .iter()
        .map(|reading| integer(field(reading, "id")))
        .collect()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn imdb_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/imdb")
}

/// A disposable copy of the imdb fixture for tests that edit files.
fn scratch_copy(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsql-cli-{test}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale copy");
    }
    copy_tree(&imdb_fixture(), &dir);
    dir
}

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("fixture dir");
    for entry in std::fs::read_dir(source).expect("fixture readable") {
        let entry = entry.expect("fixture entry");
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("fixture file copies");
        }
    }
}

#[test]
fn validate_reports_counts_and_succeeds_on_a_clean_project() {
    let output = dsql(&imdb_fixture(), &["validate"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    insta::assert_snapshot!(stdout(&output));
}

#[test]
fn catalog_validate_skips_documents_and_reports_the_effective_catalog() {
    let project = scratch_copy("catalog-validate");
    std::fs::write(
        project.join("dsql/queries/titles.dsql"),
        "this document is deliberately invalid",
    )
    .expect("break query document");
    let output = dsql(&project, &["catalog", "validate"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    insta::assert_snapshot!(stdout(&output));
}

#[test]
fn operation_list_uses_scopes_and_the_visible_alias() {
    let listed = dsql(&imdb_fixture(), &["operation", "list"]);
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    assert_eq!(stdout(&listed), "default\tTitles\n");

    let scoped = dsql(&imdb_fixture(), &["op", "list", "--scope", "missing"]);
    assert!(scoped.status.success(), "stderr: {}", stderr(&scoped));
    assert!(stdout(&scoped).is_empty());
}

#[test]
fn operation_execute_rejects_duplicate_binding_sources_before_loading_the_project() {
    let output = dsql(
        &imdb_fixture(),
        &[
            "operation",
            "execute",
            "Titles",
            "--scope",
            "default",
            "--variables",
            "{}",
            "--variables-file",
            "values.json",
        ],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be used with"),
        "got {}",
        stderr(&output)
    );
}

#[test]
fn operation_execute_validates_inputs_before_connecting() {
    let observatory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/observatory");
    let output = dsql(
        &observatory,
        &[
            "operation",
            "execute",
            "TypedReading",
            "--scope",
            "api",
            "--context",
            r#"{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a301","can_read_payload":true}"#,
        ],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("required operation input `params.confidence` was not provided"),
        "input validation should precede the placeholder database URL: {}",
        stderr(&output)
    );
}

#[test]
fn observatory_operation_list_exposes_importing_scopes() {
    let observatory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/observatory");
    let listed = dsql(&observatory, &["operation", "list"]);
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let mut operations = stdout(&listed)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    operations.sort();
    assert_eq!(
        operations,
        [
            "analytics\tConfidenceGroups",
            "analytics\tFlatReadingSummary",
            "analytics\tReadingSummary",
            "api\tContainedSensorWindow",
            "api\tDynamicReadingSearch",
            "api\tEmptyAggregateCount",
            "api\tEmptyAggregateMinimum",
            "api\tLiftedSensorWindow",
            "api\tManualFilterProbe",
            "api\tMappedSensorWindow",
            "api\tMissingFlattened",
            "api\tMultiRootOverview",
            "api\tNamespacedSensorWindow",
            "api\tNetworkTopology",
            "api\tOptionalLikeProbe",
            "api\tOptionalPredicateProbe",
            "api\tPrivacyProbe",
            "api\tRecentReadings",
            "api\tTypedReading",
        ]
    );

    let analytics = dsql(&observatory, &["op", "list", "--scope", "analytics"]);
    assert!(analytics.status.success(), "stderr: {}", stderr(&analytics));
    assert_eq!(
        stdout(&analytics),
        "analytics\tConfidenceGroups\nanalytics\tFlatReadingSummary\nanalytics\tReadingSummary\n"
    );
}

#[test]
fn observatory_project_contract_stays_in_sync() {
    let observatory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/observatory/dsql");
    let scratch = tempfile::tempdir().expect("scratch directory");
    let dsql_dir = scratch.path().join("dsql");
    std::fs::create_dir(&dsql_dir).expect("dsql directory");
    for file in ["dsql.toml", "project.generated.ts"] {
        std::fs::copy(observatory.join(file), dsql_dir.join(file))
            .expect("observatory contract input copied");
    }

    let synced = dsql(scratch.path(), &["project", "sync"]);
    assert!(synced.status.success(), "stderr: {}", stderr(&synced));
    assert!(
        stdout(&synced).starts_with("unchanged "),
        "the checked-in observatory contract is stale: {}",
        stdout(&synced)
    );
}

#[test]
fn observatory_operations_execute_with_variants_policies_and_composite_relations() {
    let Ok(database_url) = std::env::var("DSQL_OBSERVATORY_DATABASE_URL") else {
        eprintln!("DSQL_OBSERVATORY_DATABASE_URL not set; skipping live operation test");
        return;
    };
    let observatory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/observatory");
    let tenant = r#"{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a301"}"#;
    let visible = r#"{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a301","can_read_payload":true}"#;
    let hidden = r#"{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a301","can_read_payload":false}"#;
    let typed_variables = r#"{"params":{"network":"aurora","since":"2024-01-01T00:00:00Z","minimum":"-20","confidence":0.01,"flagged":false,"payload":{"sequence":2}}}"#;

    let topology = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "NetworkTopology",
        &["--context", tenant],
    );
    insta::assert_snapshot!("observatory_topology", topology);

    let multi_root = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "MultiRootOverview",
        &["--context", tenant],
    );
    insta::assert_snapshot!("observatory_multi_root_overview", multi_root);

    let readings = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "RecentReadings",
        &[
            "--variables-file",
            "inputs/recent.json",
            "--context-file",
            "inputs/no-payload.json",
        ],
    );
    insta::assert_snapshot!("observatory_recent_readings", readings);

    let defaulted_readings = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "RecentReadings",
        &["--context-file", "inputs/no-payload.json"],
    );
    // The file form intentionally mirrors the header defaults, proving that
    // omission and an explicit payload materialize the same operation.
    assert_eq!(output_json(&defaulted_readings), output_json(&readings));

    let summary = execute_observatory(
        &observatory,
        &database_url,
        "analytics",
        "ReadingSummary",
        &["--context", tenant],
    );
    insta::assert_snapshot!("observatory_reading_summary", summary);

    let empty_summary = execute_observatory(
        &observatory,
        &database_url,
        "analytics",
        "ReadingSummary",
        &[
            "--context",
            r#"{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a399"}"#,
        ],
    );
    insta::assert_snapshot!("observatory_empty_summary", empty_summary);

    let flat_summary = execute_observatory(
        &observatory,
        &database_url,
        "analytics",
        "FlatReadingSummary",
        &["--context", tenant],
    );
    insta::assert_snapshot!("observatory_flat_summary", flat_summary);

    let typed = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "TypedReading",
        &["--variables", typed_variables, "--context", visible],
    );
    insta::assert_snapshot!("observatory_typed_reading", typed);

    let restricted = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "TypedReading",
        &["--variables", typed_variables, "--context", hidden],
    );
    insta::assert_snapshot!("observatory_restricted_singular", restricted);

    let probe = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "PrivacyProbe",
        &[
            "--variables",
            r#"{"params":{"confidence":0.02,"confidences":[0.02,0.04]}}"#,
            "--context",
            hidden,
        ],
    );
    assert!(field(&output_json(&probe), "readings").is_null());

    let visible_groups = execute_observatory(
        &observatory,
        &database_url,
        "analytics",
        "ConfidenceGroups",
        &["--context", visible],
    );
    let visible_groups = output_json(&visible_groups);
    let visible_groups = field(&visible_groups, "readings")
        .as_array()
        .expect("visible confidence groups");
    assert_eq!(visible_groups.len(), 5);
    assert!(visible_groups.iter().any(|group| {
        field(group, "confidence").is_null() && integer(field(group, "count")) == 2
    }));

    let hidden_groups = execute_observatory(
        &observatory,
        &database_url,
        "analytics",
        "ConfidenceGroups",
        &["--context", hidden],
    );
    assert_eq!(
        field(&output_json(&hidden_groups), "readings"),
        &value!([{"confidence": null, "count": 6}])
    );

    let unfiltered = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "ManualFilterProbe",
        &["--context", tenant],
    );
    assert_eq!(
        integer(field(field(&output_json(&unfiltered), "readings"), "id")),
        2
    );

    let filtered = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "ManualFilterProbe",
        &[
            "--variables",
            r#"{"params":{"onlyFlagged":true}}"#,
            "--context",
            tenant,
        ],
    );
    assert_eq!(
        integer(field(field(&output_json(&filtered), "readings"), "id")),
        4
    );

    let optional_absent = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "OptionalPredicateProbe",
        &["--context", tenant],
    );
    assert_eq!(observatory_root_reading_ids(&optional_absent), [4, 8, 12]);

    let optional_present = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "OptionalPredicateProbe",
        &[
            "--variables",
            r#"{"params":{"minimum":10}}"#,
            "--context",
            tenant,
        ],
    );
    assert_eq!(
        observatory_root_reading_ids(&optional_present),
        [4, 8, 10, 12]
    );

    let optional_like = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "OptionalLikeProbe",
        &[
            "--variables",
            r#"{"params":{"code":null}}"#,
            "--context",
            tenant,
        ],
    );
    assert_eq!(
        string(field(
            field(&output_json(&optional_like), "sensors"),
            "code"
        )),
        "humidity"
    );

    let dynamic = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "DynamicReadingSearch",
        &[
            "--variables",
            r#"{"params":{"search":{"flagged":{"eq":true}},"order":[{"id":"desc"}]}}"#,
            "--context",
            visible,
        ],
    );
    assert_eq!(observatory_root_reading_ids(&dynamic), [12, 8, 4]);

    for operation in [
        "ContainedSensorWindow",
        "LiftedSensorWindow",
        "NamespacedSensorWindow",
    ] {
        let window = execute_observatory(
            &observatory,
            &database_url,
            "api",
            operation,
            &["--context", visible],
        );
        assert_eq!(observatory_reading_ids(&window), [12, 10]);
    }

    let uncapped = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "LiftedSensorWindow",
        &[
            "--variables",
            r#"{"params":{"reading_limit":null}}"#,
            "--context",
            visible,
        ],
    );
    assert_eq!(observatory_reading_ids(&uncapped).len(), 6);

    let namespaced = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "NamespacedSensorWindow",
        &[
            "--variables",
            r#"{"params":{"window_input":{"readings":{"clause":{"where":{"recorded_at":{"recorded_after":"2024-01-01T00:04:00Z"}}}}},"window_params":{"reading_limit":2,"reading_direction":"asc"}}}"#,
            "--context",
            visible,
        ],
    );
    assert_eq!(observatory_reading_ids(&namespaced), [4, 6]);

    let mapped = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "MappedSensorWindow",
        &[
            "--variables",
            r#"{"input":{"sensors":{"clause":{"where":{"recorded_after":{"recorded_after":"2024-01-01T00:04:00Z"}},"limit":{"page_size":2}}}},"params":{"sort":"asc"}}"#,
            "--context",
            visible,
        ],
    );
    assert_eq!(observatory_reading_ids(&mapped), [4, 6]);

    let empty_count = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "EmptyAggregateCount",
        &["--context", tenant],
    );
    assert_eq!(
        string(field(field(&output_json(&empty_count), "sensors"), "code")),
        "humidity"
    );

    let empty_minimum = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "EmptyAggregateMinimum",
        &[
            "--variables",
            r#"{"params":{"never":"2024-01-01T00:00:00Z"}}"#,
            "--context",
            tenant,
        ],
    );
    assert!(field(&output_json(&empty_minimum), "sensors").is_null());

    let missing_flattened = execute_observatory(
        &observatory,
        &database_url,
        "api",
        "MissingFlattened",
        &["--context", tenant],
    );
    assert!(field(&output_json(&missing_flattened), "missing_id").is_null());
}

#[test]
fn validate_fails_on_error_diagnostics() {
    let dir = scratch_copy("validate-broken");
    std::fs::write(
        dir.join("dsql/queries/broken.dsql"),
        "query Broken {\n  title(limit 1) {\n    bogus\n  }\n}\n",
    )
    .expect("broken doc");

    let output = dsql(&dir, &["validate"]);
    assert!(!output.status.success(), "errors must fail validation");
    let stdout = stdout(&output);
    assert!(stdout.contains("bogus"), "diagnostic prints, got {stdout}");
    assert!(stdout.contains("2 documents"), "counts print, got {stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lock_updates_matches_and_locked_validation_refuses_drift() {
    let dir = scratch_copy("match-lock");
    std::fs::write(
        dir.join("dsql/queries/access.dsql"),
        "filter VisibleTitles on title { where .id > 0 }\n",
    )
    .expect("filter document");

    let update = dsql(&dir, &["lock"]);
    assert!(update.status.success(), "stderr: {}", stderr(&update));
    assert!(stdout(&update).contains("dsql.lock: updated"));
    let lock_path = dir.join("dsql/dsql.lock");
    assert!(
        std::fs::read_to_string(&lock_path)
            .expect("match lock written")
            .contains("VisibleTitles")
    );

    let accepted = dsql(&dir, &["validate", "--locked"]);
    assert!(accepted.status.success(), "stderr: {}", stderr(&accepted));

    std::fs::write(&lock_path, "version: 1\nfilters: []\n").expect("stale lock");
    let stale = dsql(&dir, &["validate", "--locked"]);
    assert!(
        !stale.status.success(),
        "stale matches must fail validation"
    );
    assert!(
        stderr(&stale).contains("accepted filter matches are stale")
            && stderr(&stale).contains("run `dsql lock`"),
        "got {}",
        stderr(&stale),
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sql_prints_configured_operations_in_output_order() {
    let dir = scratch_copy("sql");
    std::fs::write(
        dir.join("dsql/queries/actors.dsql"),
        "query Actors {\n  actors: title(limit 1) {\n    id\n    title\n  }\n}\n",
    )
    .expect("second query");

    let output = dsql(&dir, &["sql"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    let actors = stdout.find("-- Actors").expect("Actors SQL heading");
    let title = stdout.find("-- Titles").expect("Titles SQL heading");
    assert!(actors < title, "SQL headings are sorted, got {stdout}");
    insta::assert_snapshot!(stdout);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sql_rejects_error_diagnostics_without_printing_partial_sql() {
    let dir = scratch_copy("sql-broken");
    std::fs::write(
        dir.join("dsql/queries/broken.dsql"),
        "query Broken {\n  title(limit 1) {\n    bogus\n  }\n}\n",
    )
    .expect("broken query");

    let output = dsql(&dir, &["sql"]);
    assert!(!output.status.success(), "errors must fail SQL generation");
    let stdout = stdout(&output);
    assert!(stdout.contains("bogus"), "diagnostic prints, got {stdout}");
    assert!(
        !stdout.lines().any(|line| line.starts_with("-- ")),
        "partial SQL must not print, got {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_rejects_duplicate_anonymous_variables() {
    let dir = scratch_copy("check-anonymous-variables");
    std::fs::write(
        dir.join("dsql/queries/anonymous.dsql"),
        "query Ambiguous {\n  title(where .id > $ and .id < $ limit 1) {\n    id\n  }\n}\n",
    )
    .expect("ambiguous document");

    let output = dsql(&dir, &["check"]);
    assert!(
        !output.status.success(),
        "duplicate anonymous variables must fail check"
    );
    let stdout = stdout(&output);
    assert!(
        stdout.contains("multiple anonymous variables")
            && stdout.contains("input.title.clause.where.id"),
        "the diagnostic explains how to disambiguate, got {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_narrows_to_one_file_and_fails_only_on_errors() {
    let dir = scratch_copy("check-file");
    std::fs::write(
        dir.join("dsql/queries/broken.dsql"),
        "query Broken {\n  title(limit 1) {\n    bogus\n  }\n}\n",
    )
    .expect("broken doc");

    // The clean document reports nothing even while its sibling is broken;
    // cross-file resolution stays active (the project still loads whole).
    let clean = dsql(&dir, &["check", "dsql/queries/titles.dsql"]);
    assert!(clean.status.success(), "stderr: {}", stderr(&clean));
    assert!(stdout(&clean).contains("no diagnostics"));

    let broken = dsql(&dir, &["check", "dsql/queries/broken.dsql"]);
    assert!(!broken.status.success(), "errors fail the selected file");
    assert!(stdout(&broken).contains("bogus"));

    let whole = dsql(&dir, &["check"]);
    assert!(!whole.status.success(), "errors fail the whole project");

    let outside = dsql(&dir, &["check", "dsql/dsql.toml"]);
    assert!(!outside.status.success());
    assert!(
        stderr(&outside).contains("not a project document"),
        "got {}",
        stderr(&outside)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fmt_touches_only_the_selected_file() {
    let dir = scratch_copy("fmt-file");
    let selected = dir.join("dsql/queries/titles.dsql");
    let other = dir.join("dsql/queries/other.dsql");
    // Both files carry the same de-formatted text; only one may change.
    let ugly = "query  Q1 {\n    title( limit 1 ) {\n     id\n  }\n}\n";
    let selected_before = ugly.replace("Q1", "Titles");
    std::fs::write(&selected, &selected_before).expect("selected doc");
    std::fs::write(&other, ugly.replace("Q1", "Other")).expect("other doc");
    let other_before = std::fs::read_to_string(&other).expect("other readable");

    let output = dsql(&dir, &["fmt", "dsql/queries/titles.dsql"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("reformatted"));

    let other_after = std::fs::read_to_string(&other).expect("other readable");
    assert_eq!(other_before, other_after, "unselected files stay untouched");
    assert_ne!(
        std::fs::read_to_string(&selected).expect("selected readable"),
        selected_before,
        "the selected file reformats"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_prints_the_cst() {
    let dir = scratch_copy("parse");
    let output = dsql(&dir, &["parse", "dsql/queries/titles.dsql"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    insta::assert_snapshot!(stdout(&output));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn metadata_commands_print_the_consumer_contract() {
    let dir = imdb_fixture();
    let schema = dsql(&dir, &["metadata-schema"]);
    assert!(schema.status.success());
    insta::assert_snapshot!("metadata_schema", stdout(&schema));

    let typescript = dsql(&dir, &["metadata-typescript"]);
    assert!(typescript.status.success());
    insta::assert_snapshot!("metadata_typescript", stdout(&typescript));
}

#[test]
fn project_sync_recreates_the_config_only_typescript_contract() {
    let dir = scratch_copy("project-sync");
    let first = dsql(&dir, &["project", "sync"]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));
    assert!(stdout(&first).contains("project.generated.ts"));
    let path = dir.join("dsql/project.generated.ts");
    let initial = std::fs::read_to_string(&path).expect("project contract written");
    assert!(initial.contains("targets: [\"default\"]"));

    let unchanged = dsql(&dir, &["project", "sync"]);
    assert!(unchanged.status.success(), "stderr: {}", stderr(&unchanged));
    assert!(stdout(&unchanged).starts_with("unchanged "));

    std::fs::write(
        dir.join("dsql/dsql.toml"),
        indoc::indoc! {r#"
            database_url = "unused"

            [resolution.shared]
            documents = [{ resolver = "dsql", paths = ["queries/**/*.dsql"] }]

            [resolution.frontend]
            documents = []
            imports = ["shared"]
        "#},
    )
    .expect("changed scope graph");
    let changed = dsql(&dir, &["project", "sync"]);
    assert!(changed.status.success(), "stderr: {}", stderr(&changed));
    let current = std::fs::read_to_string(path).expect("project contract refreshed");
    assert_ne!(initial, current);
    assert!(current.contains("targets: [\"frontend\"]"));
    assert!(current.contains("[\"shared\"]: { imports: [] }"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn generate_typescript_metadata_writes_the_contract_files() {
    let dir = scratch_copy("generate-ts-metadata");
    let out = dir.join("generated");
    let output = dsql(
        &dir,
        &[
            "generate",
            "--target",
            "typescript-metadata",
            "--out-dir",
            "generated",
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let schema_file = std::fs::read_to_string(out.join("build-manifest.schema.json"))
        .expect("schema file written");
    let types_file = std::fs::read_to_string(out.join("metadata.ts")).expect("types written");
    // The files carry exactly what the print commands emit.
    assert_eq!(
        schema_file,
        stdout(&dsql(&dir, &["metadata-schema"])),
        "schema file matches metadata-schema"
    );
    assert_eq!(
        types_file,
        stdout(&dsql(&dir, &["metadata-typescript"])),
        "types file matches metadata-typescript"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn generate_rejects_mismatched_target_arguments() {
    let dir = imdb_fixture();
    let out_dir = dsql(&dir, &["generate", "--target", "project", "--out-dir", "x"]);
    assert!(!out_dir.status.success());
    assert!(stderr(&out_dir).contains("--out-dir only applies"));

    let limit = dsql(
        &dir,
        &[
            "generate",
            "--target",
            "typescript-metadata",
            "--collection-limit",
            "5",
        ],
    );
    assert!(!limit.status.success());
    assert!(stderr(&limit).contains("--collection-limit only applies"));

    let locked = dsql(
        &dir,
        &["generate", "--target", "typescript-metadata", "--locked"],
    );
    assert!(!locked.status.success());
    assert!(stderr(&locked).contains("--locked only applies"));
}

fn scoped_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped")
}

#[test]
fn validate_rejects_artifact_collisions_without_writing() {
    let dir = std::env::temp_dir().join(format!("dsql-cli-collision-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    // Two scopes each define `query Titles`: linguistically legal, but
    // both artifacts normalize to one build path.
    std::fs::create_dir_all(dir.join("dsql")).expect("dirs");
    copy_tree(
        &imdb_fixture().join("dsql/schema"),
        &dir.join("dsql/schema"),
    );
    std::fs::write(
        dir.join("dsql/dsql.toml"),
        "database_url = \"<x>\"\ndefault_schema = \"public\"\n\n\
         [resolution.a]\ndocuments = [{ resolver = \"dsql\", paths = [\"a\"] }]\n\n[resolution.b]\ndocuments = [{ resolver = \"dsql\", paths = [\"b\"] }]\n",
    )
    .expect("config");
    let query = "query Titles {\n  title(limit 1) {\n    id\n  }\n}\n";
    std::fs::create_dir_all(dir.join("a")).expect("dirs");
    std::fs::create_dir_all(dir.join("b")).expect("dirs");
    std::fs::write(dir.join("a/q.dsql"), query).expect("doc");
    std::fs::write(dir.join("b/q.dsql"), query).expect("doc");

    let output = dsql(&dir, &["validate"]);
    assert!(!output.status.success(), "collisions must fail validation");
    assert!(
        stderr(&output).contains("both write"),
        "the collision names both artifacts, got {}",
        stderr(&output)
    );
    assert!(
        !dir.join("build").exists() && !dir.join("dsql/build").exists(),
        "validate must not write a build tree"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_prints_warnings_without_failing() {
    let dir = scratch_copy("check-warnings");
    let config = dir.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str("\n[lint]\nunindexed_scan_severity = \"warning\"\n");
    std::fs::write(&config, raw).expect("config with lints");
    std::fs::write(
        dir.join("dsql/queries/scan.dsql"),
        "query Scan {\n  title(where .kind_type.kind == \"x\" limit 1) {\n    id\n  }\n}\n",
    )
    .expect("scanning doc");

    let output = dsql(&dir, &["check"]);
    assert!(
        output.status.success(),
        "warnings alone must not fail check; stderr: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("unindexed column"),
        "the warning still prints, got {}",
        stdout(&output)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hosts_check_by_projection_and_refuse_fmt() {
    let dir = std::env::temp_dir().join(format!("dsql-cli-hosts-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    copy_tree(&scoped_fixture(), &dir);
    let host = dir.join("src/components/TitlePanel.ts");
    let text = std::fs::read_to_string(&host).expect("host readable");
    std::fs::write(&host, text.replace("      kind\n", "      bogus\n")).expect("broken region");

    let check = dsql(&dir, &["check", "src/components/TitlePanel.ts"]);
    assert!(!check.status.success(), "the region error fails the host");
    assert!(
        stdout(&check).contains("bogus"),
        "region diagnostics project onto the host, got {}",
        stdout(&check)
    );

    let fmt = dsql(&dir, &["fmt", "src/components/TitlePanel.ts"]);
    assert!(!fmt.status.success());
    assert!(
        stderr(&fmt).contains("embedding host"),
        "got {}",
        stderr(&fmt)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_reports_malformed_sources_and_fails() {
    let dir = scratch_copy("parse-broken");
    std::fs::write(
        dir.join("dsql/queries/broken.dsql"),
        "query Broken {\n  title(limit\n",
    )
    .expect("broken doc");
    let output = dsql(&dir, &["parse", "dsql/queries/broken.dsql"]);
    assert!(!output.status.success(), "parse errors fail the command");
    insta::assert_snapshot!(stdout(&output));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn generate_passes_the_documented_environment_to_the_host_generator() {
    let dir = scratch_copy("generate-env");
    let config = dir.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str(
        "\n[generate.typescript]\nenabled = true\ncmd = [\"sh\", \"-c\", \"printf '%s\\\\n%s\\\\n' \\\"$DSQL_PROJECT_DIR\\\" \\\"$DSQL_MANIFEST\\\" > generator-env.txt\"]\n",
    );
    std::fs::write(&config, raw).expect("config with generator");

    let output = dsql(&dir, &["generate"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let env = std::fs::read_to_string(dir.join("generator-env.txt")).expect("generator ran");
    let mut lines = env.lines();
    let project_dir = Path::new(lines.next().expect("project dir line"));
    let manifest = Path::new(lines.next().expect("manifest line"));
    assert!(project_dir.is_absolute(), "DSQL_PROJECT_DIR is absolute");
    assert_eq!(
        std::fs::canonicalize(project_dir).expect("project dir exists"),
        std::fs::canonicalize(&dir).expect("scratch dir exists"),
        "DSQL_PROJECT_DIR names the project base"
    );
    assert!(manifest.is_absolute(), "DSQL_MANIFEST is absolute");
    let manifest_name = manifest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    assert!(
        manifest_name.starts_with("manifest.")
            && manifest_name.ends_with(".json")
            && manifest_name
                .trim_start_matches("manifest.")
                .trim_end_matches(".json")
                .parse::<u64>()
                .is_ok(),
        "DSQL_MANIFEST names the immutable per-generation manifest, got {}",
        manifest.display()
    );
    assert!(manifest.is_file(), "the manifest was written first");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn init_scaffolds_and_refuses_to_overwrite() {
    let dir = std::env::temp_dir().join(format!("dsql-cli-init-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    std::fs::create_dir_all(&dir).expect("dir");

    let first = dsql(&dir, &["init"]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));
    assert!(dir.join("dsql/dsql.toml").is_file());
    assert!(dir.join("dsql/schema").is_dir());

    let second = dsql(&dir, &["init"]);
    assert!(!second.status.success(), "re-init must refuse");
    assert!(stderr(&second).contains("already exists"));

    std::fs::remove_dir_all(&dir).ok();
}
