//! End-to-end CLI tests: the built `dsql` binary runs against fixture
//! projects, pinning flag parsing, stdout, and exit codes — the surface
//! bowl-level tests cannot see.

use std::path::{Path, PathBuf};
use std::process::Output;

fn dsql(dir: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_dsql"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("dsql runs")
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
         [resolution.a]\ndocuments = [\"a\"]\n\n[resolution.b]\ndocuments = [\"b\"]\n",
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
    assert!(
        manifest.ends_with("dsql/build/manifest.json"),
        "DSQL_MANIFEST names the manifest, got {}",
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
