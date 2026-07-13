use std::path::{Path, PathBuf};

use dsql_generate::{GenerateOptions, generate_project};
use dsql_project::Project;

/// Copies the dsql-project scoped fixture into a temp dir so generation
/// can write its build/ tree without polluting the repository.
async fn fixture_project(test: &str) -> (PathBuf, Project) {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
    let dir = std::env::temp_dir().join(format!("dsql-generate-{test}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fixture copy");
    }
    copy_tree(&source, &dir);
    let project = Project::load_from(&dir)
        .await
        .expect("fixture project loads");
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

#[tokio::test]
async fn generates_manifest_and_artifacts() {
    let (dir, project) = fixture_project("artifacts").await;
    let output = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
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
    .await
    .expect("rerun succeeds");
    assert!(
        rerun.written.is_empty(),
        "unchanged artifacts must be skipped"
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

#[tokio::test]
async fn error_diagnostics_fail_generation() {
    let (dir, project) = fixture_project("diagnostics").await;
    std::fs::write(
        dir.join("queries/frontend/broken.dsql"),
        "query Broken {\n  missing_table {\n    id\n  }\n}\n",
    )
    .expect("write broken query");

    let error = generate_project(&project, GenerateOptions::default())
        .await
        .expect_err("diagnostics must fail generation");
    assert!(
        error.to_string().contains("missing_table"),
        "unexpected error: {error}"
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

/// Two *independent* scopes may each define an operation with the same
/// public name — resolution namespaces are separate, and without an
/// import relationship the language checks rightly stay silent (importing
/// scopes now get a check-time collision diagnostic instead) — but the
/// build tree currently uses a flat per-kind namespace: generation must
/// refuse before writing rather than let the later artifact overwrite
/// the earlier one.
#[tokio::test]
async fn colliding_operation_names_refuse_before_writing() {
    let (dir, _) = fixture_project("collide").await;
    // An `api` scope with no imports: linguistically independent of
    // `shared`, colliding only in the flat artifact namespace.
    let config = dir.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str("\n[resolution.api]\ndocuments = [\"queries/api/**/*.dsql\"]\n");
    std::fs::write(&config, raw).expect("config with api scope");
    std::fs::create_dir_all(dir.join("queries/api")).expect("api dir");
    let query = "query Collide {\n  title(limit 1) {\n    id\n  }\n}\n";
    std::fs::write(dir.join("queries/shared/collide.dsql"), query).expect("shared collide");
    std::fs::write(dir.join("queries/api/collide.dsql"), query).expect("api collide");
    let project = Project::load_from(&dir).await.expect("project reloads");

    let error = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect_err("colliding names must refuse generation");

    let message = error.to_string();
    assert!(
        message.contains("Collide") && message.contains("both write"),
        "collision error names the artifacts, got: {message}"
    );
    assert!(
        message.contains("shared/collide.dsql") && message.contains("api/collide.dsql"),
        "collision error names both sources, got: {message}"
    );
    assert!(
        !project.root.join("build").exists(),
        "no build tree may be written on collision"
    );
}

/// Names differing only by case are distinct to the language but alias
/// one file on case-insensitive filesystems, so they collide too.
#[tokio::test]
async fn case_folded_operation_names_refuse_before_writing() {
    let (dir, _) = fixture_project("case-collide").await;
    std::fs::write(
        dir.join("queries/shared/upper.dsql"),
        "query Collide {
  title(limit 1) {
    id
  }
}
",
    )
    .expect("shared upper");
    std::fs::write(
        dir.join("queries/frontend/lower.dsql"),
        "query collide {
  title(limit 1) {
    id
  }
}
",
    )
    .expect("frontend lower");
    let project = Project::load_from(&dir).await.expect("project reloads");

    let error = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect_err("case-folded names must refuse generation");

    let message = error.to_string();
    assert!(
        message.contains("Collide") && message.contains("collide"),
        "collision error names both artifacts, got: {message}"
    );
    assert!(
        !project.root.join("build").exists(),
        "no build tree may be written on collision"
    );
}
