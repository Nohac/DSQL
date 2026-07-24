//! The introspection sink: dry runs render YAML and write nothing; real
//! runs write the schema directory. The database call itself is out of
//! test scope — the sink is where the modes diverge.

use std::path::{Path, PathBuf};

use dsql_cli::commands::{publish_introspection, sink_metadata};

fn fixture_schema() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/imdb/dsql/schema")
}

#[tokio::test]
async fn overlay_validation_orders_dry_run_and_published_introspection() {
    let scratch = tempfile::tempdir().expect("scratch project");
    let dsql = scratch.path().join("dsql");
    std::fs::create_dir_all(dsql.join("overlays")).expect("project directories");
    std::fs::write(dsql.join("dsql.toml"), "database_url = \"x\"\n").expect("project config");
    let metadata = dsql_project::load_metadata_dir(&fixture_schema())
        .await
        .expect("fixture schema loads");
    dsql_project::store_metadata_dir(&metadata, &dsql.join("schema"))
        .await
        .expect("initial schema publishes");
    std::fs::write(
        dsql.join("overlays/title.yaml"),
        "version: 1\nobjects:\n  - target: { schema: public, name: title }\n    description: Overlay title.\n",
    )
    .expect("overlay");
    let project = dsql_project::Project::load_from(scratch.path())
        .await
        .expect("project loads");

    let mut structurally_invalid = metadata.clone();
    let schema = structurally_invalid
        .schemas
        .iter_mut()
        .find(|schema| schema.name == "public")
        .expect("public schema");
    let duplicate = schema.tables.first().expect("fixture table").clone();
    schema.tables.push(duplicate);
    let structural_error = publish_introspection(&project, &structurally_invalid, false)
        .await
        .expect_err("provider-invalid candidates fail before publication");
    assert!(
        structural_error.to_string().contains("duplicate table"),
        "provider structural error is retained: {structural_error}"
    );
    let unchanged = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("provider-invalid candidate leaves generated catalog untouched");
    assert_eq!(
        unchanged, metadata,
        "structural validation precedes publication"
    );

    let mut candidate = metadata.clone();
    for schema in &mut candidate.schemas {
        schema.tables.retain(|table| table.name != "title");
    }

    let dry_error = publish_introspection(&project, &candidate, true)
        .await
        .expect_err("dry-run validates overlays");
    assert!(
        dry_error.to_string().contains("title"),
        "stale overlay is reported: {dry_error}"
    );
    let unchanged = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("dry-run leaves generated catalog untouched");
    assert!(
        unchanged
            .schemas
            .iter()
            .flat_map(|schema| &schema.tables)
            .any(|table| table.name == "title")
    );

    let published_error = publish_introspection(&project, &candidate, false)
        .await
        .expect_err("published candidate may leave overlays stale");
    assert!(published_error.to_string().contains("title"));
    let published = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("new generated snapshot remains readable");
    assert!(
        published
            .schemas
            .iter()
            .flat_map(|schema| &schema.tables)
            .all(|table| table.name != "title"),
        "normal introspection commits provider truth before reporting stale overlays"
    );
}

#[tokio::test]
async fn dry_runs_render_yaml_without_writing() {
    {
        let metadata = dsql_project::load_metadata_dir(&fixture_schema())
            .await
            .expect("fixture schema loads");
        let target = std::env::temp_dir().join(format!("dsql-dry-run-{}", std::process::id()));
        if target.exists() {
            std::fs::remove_dir_all(&target).expect("clean stale dir");
        }

        let rendered = sink_metadata(&metadata, &target, true)
            .await
            .expect("dry run succeeds")
            .expect("dry run renders");
        assert!(
            rendered.contains("title") && rendered.contains("kind_type"),
            "the YAML carries the fixture tables"
        );
        assert!(!target.exists(), "dry runs must not write");

        let written = sink_metadata(&metadata, &target, false)
            .await
            .expect("real run succeeds");
        assert!(written.is_none(), "real runs print nothing");
        let reloaded = dsql_project::load_metadata_dir(&target)
            .await
            .expect("written schema loads");
        let mut canonical = metadata.clone();
        canonical.canonicalize();
        assert_eq!(reloaded, canonical, "the schema directory round-trips");

        std::fs::remove_dir_all(&target).expect("cleanup");
    }
}
