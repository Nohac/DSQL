//! Plan and SQL generation: fixture queries plan and render to PostgreSQL
//! on demand.

use std::env;
use std::path::Path;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{PlanDemand, SqlDemand};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use futures::executor::block_on;
use sqlx::{AssertSqlSafe, PgPool, Row, postgres::PgPoolOptions};

use crate::{fixture, imdb_catalog, queries_dir};

async fn sql_bowl(catalog: Catalog) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
        .await;
    bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
        .await;
    // Bound nested collections at 10, the reference snapshot setting.
    bowl.insert((
        Singleton::<SqlOptions>::new(),
        SqlOptions {
            collection_limit: Some(10),
        },
    ))
    .await;
    bowl
}

/// Renders all generated SQL facts, one section per query.
async fn render_sql(bowl: &Bowl) -> String {
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let mut generated: Vec<&GeneratedSqlFact> =
        rows.collect().into_iter().map(|(_, fact)| fact).collect();
    generated.sort_by(|left, right| left.0.output_name.cmp(&right.0.output_name));
    generated
        .into_iter()
        .map(|fact| {
            let mut section = format!("-- {}\n{}", fact.0.output_name, fact.0.sql);
            if !fact.0.parameters.is_empty() {
                let parameters: Vec<&str> = fact
                    .0
                    .parameters
                    .iter()
                    .map(|parameter| parameter.path.as_str())
                    .collect();
                section.push_str(&format!("\n-- parameters: {}", parameters.join(", ")));
            }
            if !fact.0.variants.is_empty() {
                for variant in &fact.0.variants {
                    let cases: Vec<String> = variant
                        .cases
                        .iter()
                        .map(|case| format!("{}=>{}", case.value, case.text))
                        .collect();
                    section.push_str(&format!(
                        "\n-- variant {}: {}",
                        variant.path,
                        cases.join(", ")
                    ));
                }
            }
            section
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn fixture_sql(name: &str) -> String {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(&bowl, name, &fixture(name)).await;
    render_sql(&bowl).await
}

#[test]
fn title_basic_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-title-basic.dsql").await);
    });
}

#[test]
fn movie_info_basic_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-movie-info-basic.dsql").await);
    });
}

#[test]
fn relation_path_selector_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-relation-path-selector.dsql").await);
    });
}

#[test]
fn scoped_relation_predicate_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-scoped-relation-predicate.dsql").await);
    });
}

#[test]
fn fragment_spread_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-fragment-spread.dsql").await);
    });
}

#[test]
fn variables_render_as_parameters_and_variants() {
    block_on(async {
        let bowl = sql_bowl(Catalog::hardcoded()).await;
        insert_source(
            &bowl,
            "params.dsql",
            "query UsersPage {\n  users(\n    where .name $$name_op[==, like] $$name and .id == $tenant\n    order by created_at $$dir\n    limit $$max\n    offset $skip\n  ) {\n    id\n    posts(limit 5) {\n      title\n    }\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_sql(&bowl).await);
    });
}

#[test]
fn plans_retire_when_demand_is_removed_sources_change() {
    block_on(async {
        let bowl = sql_bowl(imdb_catalog()).await;
        insert_source(&bowl, "q.dsql", "query Q {\n  title {\n    id\n  }\n}\n").await;

        let generated = bowl
            .scoop::<Query<(Entity, &GeneratedSqlFact)>>()
            .await
            .len();
        assert_eq!(generated, 1);

        let sources = bowl
            .scoop::<Query<(Entity, bowl::Mut<dsql_core::source::SourceText>)>>()
            .await;
        for (_, source) in sources.collect() {
            source
                .with_latest(|text| text.set_text("query Q {\n  kind_type {\n    kind\n  }\n}\n"))
                .await;
        }

        let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        let names: Vec<String> = rows
            .collect()
            .into_iter()
            .map(|(_, fact)| fact.0.output_name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["kind_type".to_string()],
            "stale SQL must retire with the edit"
        );
    });
}

/// Comparison right-hand sides that are themselves column paths: same-table
/// columns render as plain column references, and relation-path RHS columns
/// exercise the `OuterCurrent` scope inside the nested `EXISTS`.
#[test]
fn rhs_same_table_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-rhs-same-table.dsql").await);
    });
}

#[test]
fn rhs_relation_path_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-rhs-relation-path.dsql").await);
    });
}
/// Fragment bodies are expanded by walks in *other* files; the DefIndex
/// fragment fingerprint is the tracked dependency that keeps dependents
/// fresh across files.
#[test]
fn cross_file_fragment_body_edits_rederive_sql() {
    use bowl::Mut;
    use dsql_core::source::SourceText;
    block_on(async {
        let bowl = sql_bowl(imdb_catalog()).await;
        insert_source(&bowl, "frag.dsql", "fragment Bits on title {\n  id\n}\n").await;
        insert_source(
            &bowl,
            "query.dsql",
            "query UsesBits {\n  title(limit 1) {\n    ...Bits\n  }\n}\n",
        )
        .await;
        let before = render_sql(&bowl).await;
        assert!(
            before.contains("\"id\""),
            "fragment field planned: {before}"
        );

        let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (_, source) in sources.collect() {
            source
                .with_latest(|text| {
                    if text
                        .to_text()
                        .is_some_and(|text| text.contains("fragment Bits"))
                    {
                        text.set_text("fragment Bits on title {\n  title\n}\n");
                    }
                })
                .await;
        }
        let after = render_sql(&bowl).await;
        // Snapshot the whole post-edit render: a false pass from dropped
        // expansion (rather than re-derived expansion) is impossible to
        // write through a full snapshot.
        insta::assert_snapshot!(after);
    });
}

/// Every parameter-free valid fixture executes against the reference imdb
/// database when the opt-in URL is present. Ordinary test runs skip this
/// external integration boundary.
#[tokio::test]
async fn valid_query_fixtures_execute_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reference database connects");

    for name in fixture_names("valid") {
        let bowl = sql_bowl(imdb_catalog()).await;
        insert_source(&bowl, &name, &fixture(&name)).await;
        let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        let generated: Vec<String> = rows
            .collect()
            .into_iter()
            .map(|(_, fact)| {
                assert!(
                    fact.0.parameters.is_empty(),
                    "{name} must be parameter-free for live execution"
                );
                fact.0.sql.clone()
            })
            .collect();
        assert!(!generated.is_empty(), "{name} must generate SQL");
        for sql in generated {
            let value = execute_json(&pool, &sql).await;
            assert!(
                value.is_array() || value.is_object(),
                "{name} generated JSON must be an array or object"
            );
        }
    }
}

/// Data-sensitive integration fixtures pin the reference imdb outputs when
/// the opt-in database URL is present.
#[tokio::test]
async fn integration_query_fixtures_match_expected_output_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reference database connects");

    for name in fixture_names("integration") {
        let bowl = sql_bowl(imdb_catalog()).await;
        insert_source(&bowl, &name, &fixture(&name)).await;
        let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        let generated: Vec<String> = rows
            .collect()
            .into_iter()
            .map(|(_, fact)| fact.0.sql.clone())
            .collect();
        assert_eq!(generated.len(), 1, "{name} must generate one query");
        let value = execute_json(&pool, &generated[0]).await;
        let output = serde_json::to_string_pretty(&value).expect("JSON formats");
        let snapshot = Path::new(&name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("integration")
            .replace('-', "_");
        insta::assert_snapshot!(format!("{snapshot}_output"), output);
    }
}

async fn execute_json(pool: &PgPool, sql: &str) -> serde_json::Value {
    let sql = format!("select ({})::text", sql.trim().trim_end_matches(';'));
    let row = sqlx::query(AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .expect("generated SQL executes");
    let json: String = row.try_get(0).expect("query returns JSON text");
    serde_json::from_str(&json).expect("generated output is JSON")
}

fn fixture_names(directory: &str) -> Vec<String> {
    let root = queries_dir().join(directory);
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("fixture directory exists")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("dsql") {
                return None;
            }
            let file_name = path.file_name()?.to_str()?;
            Some(format!("{directory}/{file_name}"))
        })
        .collect();
    names.sort();
    names
}
