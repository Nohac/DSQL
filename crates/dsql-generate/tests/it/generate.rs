use std::path::{Path, PathBuf};

use dsql_generate::{GenerateOptions, assemble_project, generate_project};
use dsql_project::{Project, open_analysis_bowl};

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
    /// Hashes and hashed paths churn with any metadata change; the layout
    /// is what this snapshot pins.
    fn redact_hashes(manifest: &str) -> String {
        let mut redacted = String::new();
        for (index, piece) in manifest.split('"').enumerate() {
            if index > 0 {
                redacted.push('"');
            }
            if piece.len() == 64 && piece.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                redacted.push_str("<hash>");
            } else if piece.contains('.')
                && piece.split('.').nth(1).is_some_and(|address| {
                    address.len() == 16 && address.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                let mut parts = piece.split('.');
                let stem = parts.next().unwrap_or_default();
                let extension = parts.nth(1).unwrap_or_default();
                redacted.push_str(&format!("{stem}.<address>.{extension}"));
            } else {
                redacted.push_str(piece);
            }
        }
        redacted
    }

    let (dir, project) = fixture_project("artifacts").await;
    // A fragment composed of other fragments, spread at the root (empty
    // path) and inside a relation (nested path): renderers reuse types
    // from exactly this provenance.
    std::fs::write(
        dir.join("queries/shared/panels.dsql"),
        "fragment KindNameBits on kind_type {\n  kind\n}\n\n\
         fragment PanelBits on title {\n  ...TitleBits\n  panel_kind: kind_type {\n    ...KindNameBits\n  }\n}\n",
    )
    .expect("write composed fragments");
    let output = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect("generation succeeds");

    assert!(output.manifest_path.exists(), "immutable manifest exists");
    assert!(
        output.current_manifest_path.exists(),
        "the pointer manifest exists"
    );
    assert_eq!(output.generation_id, 1, "a fresh tree starts at 1");
    let manifest = std::fs::read_to_string(&output.manifest_path).expect("manifest readable");
    assert_eq!(
        manifest,
        std::fs::read_to_string(&output.current_manifest_path).expect("pointer readable"),
        "the pointer carries the same document"
    );
    insta::assert_snapshot!("manifest", redact_hashes(&manifest));

    // Consumers follow manifest entry paths — content-addressed files are
    // never discovered by globbing.
    let parsed: dsql_metadata::BuildManifest =
        facet_json::from_str(&manifest).expect("manifest parses");
    let entry_path = |name: &str| {
        parsed
            .operations
            .iter()
            .map(|entry| (entry.name.clone(), entry.path.clone()))
            .chain(
                parsed
                    .fragments
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.path.clone())),
            )
            .find(|(entry_name, _)| entry_name == name)
            .map(|(_, path)| project.root.join("build").join(path))
            .expect("manifest names the artifact")
    };
    let operation =
        std::fs::read_to_string(entry_path("Titles")).expect("operation artifact written");
    insta::assert_snapshot!("operation", operation);

    let fragment =
        std::fs::read_to_string(entry_path("TitleBits")).expect("fragment artifact written");
    insta::assert_snapshot!("fragment", fragment);

    // The composed fragment records its spread provenance: the root
    // spread at the empty path, the nested one at its relation path.
    let composed =
        std::fs::read_to_string(entry_path("PanelBits")).expect("composed fragment written");
    insta::assert_snapshot!("composed_fragment", composed);

    // The embedded operation source-maps into its host .ts file and
    // records the fragments its result paths came from.
    let embedded =
        std::fs::read_to_string(entry_path("TitlePanel")).expect("embedded artifact written");
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
        rerun
            .written
            .iter()
            .all(|path| !path.to_string_lossy().contains("operations/")
                && !path.to_string_lossy().contains("fragments/")),
        "unchanged artifact files must be skipped (manifests still commit)"
    );
    assert_eq!(
        rerun.generation_id,
        output.generation_id + 1,
        "every explicit generate commits a fresh generation"
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

#[tokio::test]
async fn duplicate_anonymous_variables_fail_before_publication() {
    let (dir, project) = fixture_project("anonymous-variables").await;
    std::fs::write(
        dir.join("queries/frontend/anonymous.dsql"),
        "query Ambiguous {\n  title(where .id > $ and .id < $ limit 1) {\n    id\n  }\n}\n",
    )
    .expect("write ambiguous query");

    let error = generate_project(&project, GenerateOptions::default())
        .await
        .expect_err("duplicate anonymous variables must fail generation");
    let message = error.to_string();
    assert!(
        message.contains("multiple anonymous variables")
            && message.contains("input.title.clause.where.id"),
        "unexpected error: {message}"
    );
    assert!(
        !project.root.join("build").exists(),
        "language errors must refuse before publication"
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

#[tokio::test]
async fn generated_scope_groups_include_transitive_import_artifacts() {
    let (dir, _) = fixture_project("transitive-groups").await;
    let config = dir.join("dsql/dsql.toml");
    let raw = std::fs::read_to_string(&config)
        .expect("config readable")
        .replace("imports = [\"shared\"]", "imports = [\"middle\"]");
    let raw = format!("{raw}\n[resolution.middle]\ndocuments = []\nimports = [\"shared\"]\n");
    std::fs::write(&config, raw).expect("transitive config");
    let project = Project::load_from(&dir).await.expect("project reloads");
    let bowl = open_analysis_bowl(&project).await.expect("bowl opens");

    let assembled = assemble_project(&bowl, &project, GenerateOptions::default())
        .await
        .expect("assembly succeeds");
    let frontend = assembled
        .snapshot
        .groups
        .iter()
        .find(|group| group.name == "frontend")
        .expect("frontend group");
    assert_eq!(frontend.imports, vec!["middle"]);
    assert!(
        frontend
            .artifacts
            .iter()
            .any(|artifact| artifact == "shared/fragment/TitleBits"),
        "frontend closure includes shared artifacts: {:?}",
        frontend.artifacts
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

#[tokio::test]
async fn numeric_wire_types_flow_through_generated_metadata() {
    let (dir, _) = fixture_project("numeric-wire").await;
    std::fs::write(
        dir.join("dsql/schema/public/metrics.yaml"),
        "---\nschema: public\nname: metrics\nobject_type: table\ncolumns:\n  - name: amount\n    database_type: numeric\n    data_type: numeric\n    not_null: true\n  - name: ratio\n    database_type: float8\n    data_type: float\n    not_null: false\nconstraints: []\nforeign_keys: []\nindexes: []\n",
    )
    .expect("numeric schema fixture");
    std::fs::write(
        dir.join("queries/frontend/numeric.dsql"),
        "query NumericMetrics {\n  metrics(where .amount >= $$minimum) {\n    amount\n    ratio\n  }\n}\n",
    )
    .expect("numeric query fixture");
    let project = Project::load_from(&dir).await.expect("project reloads");
    let bowl = open_analysis_bowl(&project).await.expect("bowl opens");
    let assembled = assemble_project(&bowl, &project, GenerateOptions::default())
        .await
        .expect("assembly succeeds");
    let artifact = assembled
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "NumericMetrics")
        .expect("numeric operation");

    insta::assert_snapshot!(artifact.serialized);

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

#[tokio::test]
async fn aggregate_objects_flow_through_operation_and_fragment_metadata() {
    let (dir, _) = fixture_project("aggregate-metadata").await;
    std::fs::write(
        dir.join("dsql/schema/public/users.yaml"),
        concat!(
            "---\n",
            "schema: public\n",
            "name: users\n",
            "object_type: table\n",
            "columns:\n",
            "  - name: id\n",
            "    database_type: uuid\n",
            "    data_type: uuid\n",
            "    not_null: true\n",
            "  - name: name\n",
            "    database_type: text\n",
            "    data_type: text\n",
            "    not_null: true\n",
            "constraints:\n",
            "  - name: users_pkey\n",
            "    kind: primary_key\n",
            "    columns: [id]\n",
            "foreign_keys: []\n",
            "indexes:\n",
            "  - name: users_pkey\n",
            "    columns: [id]\n",
            "    unique: true\n",
        ),
    )
    .expect("users schema fixture");
    std::fs::write(
        dir.join("dsql/schema/public/posts.yaml"),
        concat!(
            "---\n",
            "schema: public\n",
            "name: posts\n",
            "object_type: table\n",
            "columns:\n",
            "  - name: id\n",
            "    database_type: uuid\n",
            "    data_type: uuid\n",
            "    not_null: true\n",
            "  - name: user_id\n",
            "    database_type: uuid\n",
            "    data_type: uuid\n",
            "    not_null: true\n",
            "  - name: title\n",
            "    database_type: text\n",
            "    data_type: text\n",
            "    not_null: false\n",
            "  - name: created_at\n",
            "    database_type: timestamptz\n",
            "    data_type: timestamptz\n",
            "    not_null: true\n",
            "constraints:\n",
            "  - name: posts_pkey\n",
            "    kind: primary_key\n",
            "    columns: [id]\n",
            "foreign_keys:\n",
            "  - name: posts_user_id_fkey\n",
            "    columns: [user_id]\n",
            "    references:\n",
            "      schema: public\n",
            "      table: users\n",
            "      columns: [id]\n",
            "indexes:\n",
            "  - name: posts_pkey\n",
            "    columns: [id]\n",
            "    unique: true\n",
        ),
    )
    .expect("posts schema fixture");
    std::fs::write(
        dir.join("queries/frontend/aggregates.dsql"),
        concat!(
            "fragment UserStats on users {\n",
            "  post_stats: posts(where .title == $$title) | aggregate {\n",
            "    count\n",
            "    latest: max .created_at\n",
            "  }\n",
            "  post_groups: posts | aggregate by title_group: .title {\n",
            "    count\n",
            "    latest_group: max .created_at\n",
            "  }\n",
            "}\n",
            "fragment FlatPostStats on users {\n",
            "  ...posts(where .title == $$flat_title) | aggregate {\n",
            "    flat_post_count: count\n",
            "    flat_latest: max .created_at\n",
            "  }\n",
            "}\n",
            "query RootStats {\n",
            "  user_stats: users(where .name == $$name) | aggregate {\n",
            "    count\n",
            "    first_name: min .name\n",
            "  }\n",
            "}\n",
            "query GroupedRoot {\n",
            "  user_groups: users | aggregate by label: .name {\n",
            "    count\n",
            "    latest_name: max .name\n",
            "  }\n",
            "}\n",
            "query NestedStats {\n",
            "  users(limit 1) {\n",
            "    id\n",
            "    ...UserStats\n",
            "  }\n",
            "}\n",
            "query FlattenRoot {\n",
            "  ...users(where .name == $$flat_name) | aggregate {\n",
            "    user_count: count\n",
            "    first_name: min .name\n",
            "  }\n",
            "}\n",
            "query FlattenNested {\n",
            "  accounts: users(limit 1) {\n",
            "    id\n",
            "    ...FlatPostStats\n",
            "  }\n",
            "}\n",
            "query FlattenOwner {\n",
            "  feed: posts(limit 1) {\n",
            "    id\n",
            "    ...users(where .name == $$owner_name) {\n",
            "      owner_name: name\n",
            "      owner_posts: posts(limit 1) { title }\n",
            "      ...posts | aggregate { owner_post_count: count }\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )
    .expect("aggregate query fixture");
    let project = Project::load_from(&dir).await.expect("project reloads");
    let bowl = open_analysis_bowl(&project).await.expect("bowl opens");
    let assembled = assemble_project(
        &bowl,
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect("assembly succeeds");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| {
            matches!(
                artifact.name.as_str(),
                "FlatPostStats"
                    | "GroupedRoot"
                    | "FlattenNested"
                    | "FlattenOwner"
                    | "FlattenRoot"
                    | "NestedStats"
                    | "RootStats"
                    | "UserStats"
            )
        })
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));

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
    raw.push_str(
        "\n[resolution.api]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/api/**/*.dsql\"] }]\n",
    );
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
