//! The introspection sink: dry runs render YAML and write nothing; real
//! runs write the schema directory. The database call itself is out of
//! test scope — the sink is where the modes diverge.

use std::path::{Path, PathBuf};

use dsql_cli::commands::sink_metadata;

fn fixture_schema() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/imdb/dsql/schema")
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
