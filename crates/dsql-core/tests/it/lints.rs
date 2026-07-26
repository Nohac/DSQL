//! Lints: unindexed-scan findings are advisory, severity-configurable, and
//! absent entirely without a lint configuration.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{
    Catalog, DatabaseMetadata, SchemaMetadata, insert_catalog, table_metadata_from_yaml,
};
use dsql_core::facts::{DiagnosticsDemand, Severity};
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::insert_source;

use crate::{imdb_catalog, render_diagnostic_facts};

async fn linted_bowl(config: Option<LintConfig>) -> Bowl {
    linted_bowl_with_catalog(config, imdb_catalog()).await
}

async fn linted_bowl_with_catalog(config: Option<LintConfig>, catalog: Catalog) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    if let Some(config) = config {
        bowl.insert((Singleton::<LintConfig>::new(), config)).await;
    }
    bowl
}

#[tokio::test]
async fn included_columns_remain_unindexed_for_linting() {
    let parents = table_metadata_from_yaml(
        r#"---
schema: public
name: parents
object_type: table
columns:
  - name: id
    database_type: int4
    data_type: int
    not_null: true
constraints:
  - name: parents_pkey
    kind: primary_key
    columns: [id]
foreign_keys: []
indexes:
  - name: parents_pkey
    access_method: btree
    keys:
      - column: id
        capabilities: [equality]
    unique: true
"#,
    )
    .expect("embedded parent metadata parses");
    let records = table_metadata_from_yaml(
        r#"---
schema: public
name: records
object_type: table
columns:
  - name: parent_id
    database_type: int4
    data_type: int
    not_null: true
  - name: included_value
    database_type: int4
    data_type: int
    not_null: true
constraints: []
foreign_keys:
  - name: records_parent_id_fkey
    columns: [parent_id]
    references:
      schema: public
      table: parents
      columns: [id]
indexes:
  - name: records_parent_id_idx
    access_method: btree
    keys:
      - column: parent_id
        capabilities: [equality]
    included_columns: [included_value]
    unique: false
"#,
    )
    .expect("embedded table metadata parses");
    let catalog = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![parents, records],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("embedded catalog builds");
    let bowl = linted_bowl_with_catalog(Some(LintConfig::default()), catalog).await;
    insert_source(
        &bowl,
        "include-lint.dsql",
        indoc::indoc! {r#"
            query IncludeLint {
              parents(where .records.included_value == 1) { id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

/// `aka_title->episode_of_id` joins over `episode_of_id`, which has no
/// index; `.aka_title->movie_id.title` scans `aka_title.title`, also
/// unindexed.
const SLOW_QUERY: &str = "query Slow {\n  title(where .aka_title->movie_id.title like \"%x%\") {\n    id\n    episodes: aka_title->episode_of_id {\n      id\n    }\n  }\n}\n";

#[tokio::test]
async fn unindexed_joins_and_scans_are_flagged() {
    let bowl = linted_bowl(Some(LintConfig::default())).await;
    insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;
    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn lint_severity_follows_configuration() {
    let bowl = linted_bowl(Some(LintConfig {
        unindexed_scan_severity: Some(Severity::Warning),
    }))
    .await;
    insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;
    let rendered = render_diagnostic_facts(&bowl).await;
    assert!(rendered.contains("Warning["), "unexpected: {rendered}");
    assert!(!rendered.contains("Info["), "unexpected: {rendered}");
}

#[tokio::test]
async fn lints_are_off_without_configuration() {
    for config in [
        None,
        Some(LintConfig {
            unindexed_scan_severity: None,
        }),
    ] {
        let bowl = linted_bowl(config).await;
        insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;
        assert_eq!(render_diagnostic_facts(&bowl).await, "");
    }
}

/// The demand marker gates lints like every diagnostic stage.
#[tokio::test]
async fn lints_wait_for_diagnostics_demand() {
    let bowl = language_bowl().await;
    dsql_core::catalog::insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<LintConfig>::new(), LintConfig::default()))
        .await;
    insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;

    let rows = bowl
        .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
        .await;
    assert_eq!(rows.len(), 0, "no demand, no lints");
}

/// Root-anchored predicate paths keep the pre-resolution behavior: only
/// current-anchored relation steps lint as nested scans. This pins the
/// deliberate choice recorded in `lint_predicates` — root paths need
/// their own rule before they warn.
#[tokio::test]
async fn root_anchored_predicate_paths_do_not_lint() {
    let bowl = linted_bowl(Some(LintConfig::default())).await;
    insert_source(
            &bowl,
            "root-path.dsql",
            "query RootPath {\n  title(where ~aka_title->movie_id.title == \"x\" limit 1) {\n    id\n  }\n}\n",
        )
        .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}
