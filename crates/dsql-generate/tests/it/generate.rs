use std::path::{Path, PathBuf};

use dsql_generate::{GenerateOptions, generate_project};
use dsql_project::Project;

/// Copies the dsql-project scoped fixture into a temp dir so generation
/// can write its build/ tree without polluting the repository.
fn fixture_project(test: &str) -> (PathBuf, Project) {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
    let dir = std::env::temp_dir().join(format!("dsql-generate-{test}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fixture copy");
    }
    copy_tree(&source, &dir);
    let project = Project::load_from(&dir).expect("fixture project loads");
    (dir, project)
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
fn generates_manifest_and_artifacts() {
    let (dir, project) = fixture_project("artifacts");
    let output = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .expect("generation succeeds");

    assert!(output.manifest_path.exists());
    let manifest = std::fs::read_to_string(&output.manifest_path).expect("manifest readable");
    insta::assert_snapshot!("manifest", manifest);

    let operation = std::fs::read_to_string(project.root.join("build/operations/Titles.json"))
        .expect("operation artifact written");
    insta::assert_snapshot!("operation", operation);

    let fragment = std::fs::read_to_string(project.root.join("build/fragments/TitleBits.json"))
        .expect("fragment artifact written");
    insta::assert_snapshot!("fragment", fragment);

    // The embedded operation source-maps into its host .ts file and
    // records the fragments its result paths came from.
    let embedded = std::fs::read_to_string(project.root.join("build/operations/TitlePanel.json"))
        .expect("embedded operation artifact written");
    insta::assert_snapshot!("embedded_operation", embedded);

    // Second run: everything unchanged, nothing rewritten.
    let rerun = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .expect("rerun succeeds");
    assert!(
        rerun.written.is_empty(),
        "unchanged artifacts must be skipped"
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

#[test]
fn error_diagnostics_fail_generation() {
    let (dir, project) = fixture_project("diagnostics");
    std::fs::write(
        dir.join("queries/frontend/broken.dsql"),
        "query Broken {\n  missing_table {\n    id\n  }\n}\n",
    )
    .expect("write broken query");

    let error = generate_project(&project, GenerateOptions::default())
        .expect_err("diagnostics must fail generation");
    assert!(
        error.to_string().contains("missing_table"),
        "unexpected error: {error}"
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}
