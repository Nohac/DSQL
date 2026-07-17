//! Plan and SQL generation: fixture queries plan and render to PostgreSQL
//! on demand.

use std::collections::HashMap;
use std::env;
use std::path::Path;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{PlanDemand, SqlDemand};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use sqlx::{AssertSqlSafe, Column, Row};
use sqlx_postgres::{PgPool, PgPoolOptions};

use crate::{fixture, imdb_catalog, numeric_catalog, queries_dir, set_source_text};

async fn sql_bowl(catalog: Catalog) -> Bowl {
    sql_bowl_with_limit(catalog, Some(10)).await
}

async fn sql_bowl_with_limit(catalog: Catalog, collection_limit: Option<u64>) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
        .await;
    bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
        .await;
    bowl.insert((
        Singleton::<SqlOptions>::new(),
        SqlOptions { collection_limit },
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

const MOVIE_SIGNALS: &str = concat!(
    "fragment MovieSignals on title {\n",
    "  ...movie_info_idx(where .info_type_id == 101) | aggregate {\n",
    "    rating: max .info\n",
    "  }\n",
    "  ...movie_info_idx(where .info_type_id == 100) | aggregate {\n",
    "    votes: max .info\n",
    "  }\n",
    "}\n",
);

const RENDERER_EDGE_QUERY: &str = concat!(
    "query RendererEdges {\n",
    "  signals: title(where .id == 2 limit 1) { id ...MovieSignals }\n",
    "  ratings: movie_info_idx(\n",
    "    where .info_type_id == 101\n",
    "    order by info desc, id asc\n",
    "    limit 16\n",
    "  ) { id info }\n",
    "  ordered_title: title(where .id == 943844 limit 1) {\n",
    "    aliases: aka_title->movie_id(order by kind_id desc, id asc limit 16) {\n",
    "      id\n",
    "      kind_id\n",
    "    }\n",
    "  }\n",
    "  singular: title(where .id == 2 limit 1) {\n",
    "    id\n",
    "    ...kind_type { kind }\n",
    "  }\n",
    "  ...kind_type | aggregate {\n",
    "    kind_count: count\n",
    "    first_kind: min .kind\n",
    "  }\n",
    "}\n",
);

#[tokio::test]
async fn title_basic_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-title-basic.dsql").await);
}

#[tokio::test]
async fn movie_info_basic_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-movie-info-basic.dsql").await);
}

#[tokio::test]
async fn relation_path_selector_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-relation-path-selector.dsql").await);
}

#[tokio::test]
async fn scoped_relation_predicate_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-scoped-relation-predicate.dsql").await);
}

#[tokio::test]
async fn fragment_spread_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-fragment-spread.dsql").await);
}

#[tokio::test]
async fn variables_render_as_parameters_and_variants() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
            &bowl,
            "params.dsql",
            "query UsersPage {\n  public::users(\n    where .name $$name_op[==, like] $$name and .id == $tenant\n    order by created_at $$dir\n    limit $$max\n    offset $skip\n  ) {\n    id\n    posts(limit 5) {\n      title\n    }\n  }\n}\n",
        )
        .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn exact_and_floating_numbers_use_their_public_wire_types() {
    let bowl = sql_bowl(numeric_catalog()).await;
    insert_source(
        &bowl,
        "numeric.dsql",
        concat!(
            "query NumericMetrics {\n",
            "  metrics(where .amount >= 12345678901234567890.12345678901234567890) {\n",
            "    amount\n",
            "    ratio\n",
            "  }\n",
            "  groups: metrics | aggregate by amount_group: .amount, ratio_group: .ratio {\n",
            "    count\n",
            "    total_amount: sum .amount\n",
            "    average_ratio: avg .ratio\n",
            "  }\n",
            "  summary: metrics | aggregate {\n",
            "    total_amount: sum .amount\n",
            "    average_amount: avg .amount\n",
            "    total_ratio: sum .ratio\n",
            "    average_ratio: avg .ratio\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn numeric_template_replacements_never_rewrite_source_literals() {
    let bowl = sql_bowl_with_limit(numeric_catalog(), None).await;
    insert_source(
        &bowl,
        "numeric-template.dsql",
        concat!(
            "query NumericTemplate {\n",
            "  metrics(\n",
            "    where .amount == 9000000000000000000\n",
            "      or .amount == 9000000000000000005.5\n",
            "      or .amount == 12345678901234567890.12345678901234567890\n",
            "    limit $$page_limit\n",
            "    offset $$page_offset\n",
            "  ) { amount }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn aggregates_render_root_and_nested_objects_without_safety_caps() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "aggregate-sql.dsql",
        concat!(
            "query AggregateSql {\n",
            "  stats: public::users(where .email != null) | aggregate {\n",
            "    count\n",
            "    populated_email: count .email\n",
            "    any: exists\n",
            "    first_name: min .name\n",
            "    latest_signup: max .created_at\n",
            "  }\n",
            "  by_email: public::users | aggregate by email_group: .email {\n",
            "    count\n",
            "    earliest_signup: min .created_at\n",
            "  }\n",
            "  public::users(limit 2) {\n",
            "    id\n",
            "    post_stats: posts(where .title like \"%x%\") | aggregate {\n",
            "      count\n",
            "      latest: max .created_at\n",
            "    }\n",
            "    post_groups: posts | aggregate by title_group: .title {\n",
            "      count\n",
            "      latest: max .created_at\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn scalar_aggregate_predicates_render_correlated_values() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "aggregate-predicate-sql.dsql",
        concat!(
            "query AggregatePredicateSql {\n",
            "  title(\n",
            "    where .movie_info_idx | exists\n",
            "      and .movie_info_idx | count >= $minimum\n",
            "      and $maximum >= (.movie_info_idx | count)\n",
            "      and 0 < (.movie_info_idx | count .info)\n",
            "      and (.movie_info_idx | count .info) > 0\n",
            "      and (.movie_info_idx | count .info) <= (.movie_info_idx | count)\n",
            "      and (.movie_info_idx | min .info) like \"4.%\"\n",
            "      and (.movie_info_idx | max .info) != null\n",
            "      and (.movie_info_idx | sum .info_type_id) > 0\n",
            "      and (.movie_info_idx | avg .info_type_id) > 0\n",
            "    order by id asc\n",
            "    limit 5\n",
            "  ) { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn flattened_objects_export_fields_without_wrapper_keys() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "flattened-sql.dsql",
        concat!(
            "query FlattenOwner {\n",
            "  feed: public::posts(limit 1) {\n",
            "    id\n",
            "    ...users(where .name like $$owner) {\n",
            "      owner_name: name\n",
            "      recent: posts(limit 1) { title }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "query FlattenPostStats {\n",
            "  accounts: public::users(limit 1) {\n",
            "    id\n",
            "    ...posts(where .title like $$title) | aggregate {\n",
            "      post_count: count\n",
            "      latest_post: max .created_at\n",
            "    }\n",
            "  }\n",
            "}\n",
            "query FlattenRoot {\n",
            "  ...public::users(where .name == $$root_name) | aggregate {\n",
            "    user_count: count\n",
            "    first_name: min .name\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn repeated_flattened_aggregates_and_collection_ordering_render_independently() {
    let bowl = sql_bowl_with_limit(imdb_catalog(), None).await;
    insert_source(&bowl, "movie-signals.dsql", MOVIE_SIGNALS).await;
    insert_source(&bowl, "renderer-edges.dsql", RENDERER_EDGE_QUERY).await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn same_slot_aggregate_function_edits_rederive_sql() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    let file = insert_source(
        &bowl,
        "aggregate-edit.dsql",
        "query Edge { edge: public::users | aggregate { value: max .name } }\n",
    )
    .await;
    let before = render_sql(&bowl).await;

    set_source_text(
        &bowl,
        file,
        "query Edge { edge: public::users | aggregate { value: min .name } }\n",
    )
    .await;
    let after = render_sql(&bowl).await;

    set_source_text(
        &bowl,
        file,
        "query Edge { edge: public::users | aggregate by key: .name { count } }\n",
    )
    .await;
    let grouped_before = render_sql(&bowl).await;
    set_source_text(
        &bowl,
        file,
        "query Edge { edge: public::users | aggregate by key: .email { count } }\n",
    )
    .await;
    let grouped_after = render_sql(&bowl).await;

    insta::assert_snapshot!(format!(
        "before:\n{before}\n\nafter:\n{after}\n\ngrouped before:\n{grouped_before}\n\ngrouped after:\n{grouped_after}"
    ));
}

#[tokio::test]
async fn plans_retire_when_demand_is_removed_sources_change() {
    let bowl = sql_bowl(imdb_catalog()).await;
    let file = insert_source(&bowl, "q.dsql", "query Q {\n  title {\n    id\n  }\n}\n").await;

    let generated = bowl
        .scoop::<Query<(Entity, &GeneratedSqlFact)>>()
        .await
        .len();
    assert_eq!(generated, 1);

    set_source_text(&bowl, file, "query Q {\n  kind_type {\n    kind\n  }\n}\n").await;

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
}

/// Comparison right-hand sides that are themselves column paths: same-table
/// columns render as plain column references, and relation-path RHS columns
/// exercise the `OuterCurrent` scope inside the nested `EXISTS`.
#[tokio::test]
async fn rhs_same_table_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-rhs-same-table.dsql").await);
}

#[tokio::test]
async fn rhs_relation_path_sql() {
    insta::assert_snapshot!(fixture_sql("valid/imdb-rhs-relation-path.dsql").await);
}
/// Fragment bodies are expanded by walks in *other* files; the DefIndex
/// fragment fingerprint is the tracked dependency that keeps dependents
/// fresh across files.
#[tokio::test]
async fn cross_file_fragment_body_edits_rederive_sql() {
    let bowl = sql_bowl(imdb_catalog()).await;
    let fragment = insert_source(&bowl, "frag.dsql", "fragment Bits on title {\n  id\n}\n").await;
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

    set_source_text(&bowl, fragment, "fragment Bits on title {\n  title\n}\n").await;
    let after = render_sql(&bowl).await;
    // Snapshot the whole post-edit render: a false pass from dropped
    // expansion (rather than re-derived expansion) is impossible to
    // write through a full snapshot.
    insta::assert_snapshot!(after);
}

/// Clause resolutions belong to their clause entities, not to the query
/// file that expands them. Cross-file fragment expansion must preserve every
/// semantic clause part instead of retaining only resolution-free limits.
#[tokio::test]
async fn cross_file_fragment_clauses_are_preserved_in_sql() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "rating-fields.dsql",
        concat!(
            "fragment RatingFields on title {\n",
            "  ratings: movie_info_idx(where .info_type_id == 101 order by id asc limit 1) {\n",
            "    info\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;
    insert_source(
        &bowl,
        "top-rated.dsql",
        concat!(
            "query TopRated {\n",
            "  title(limit 1) {\n",
            "    id\n",
            "    ...RatingFields\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn renderer_edge_cases_execute_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reference database connects");
    let bowl = sql_bowl_with_limit(imdb_catalog(), None).await;
    insert_source(&bowl, "movie-signals.dsql", MOVIE_SIGNALS).await;
    insert_source(&bowl, "renderer-edges.dsql", RENDERER_EDGE_QUERY).await;
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let generated = rows
        .collect()
        .into_iter()
        .map(|(_, fact)| (fact.0.output_name.clone(), fact.0.sql.clone()))
        .collect::<HashMap<_, _>>();

    let flattened_sql = generated
        .get("kind_type")
        .expect("root flattened SQL exists")
        .clone();
    let flattened = sqlx::query(AssertSqlSafe(flattened_sql))
        .fetch_one(&pool)
        .await
        .expect("root flattened SQL executes");
    let column_names = flattened
        .columns()
        .iter()
        .map(|column| column.name())
        .collect::<Vec<_>>();
    assert_eq!(column_names, ["kind_count", "first_kind"]);
    let kind_count: serde_json::Value = flattened
        .try_get("kind_count")
        .expect("count uses the JSON driver type");
    let first_kind: serde_json::Value = flattened
        .try_get("first_kind")
        .expect("text uses the JSON driver type");
    assert!(kind_count.is_number(), "count stays a JSON number");
    assert!(first_kind.is_string(), "text stays a JSON string");

    let singular = execute_json(
        &pool,
        generated.get("singular").expect("singular SQL exists"),
    )
    .await;
    let singular = singular
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(serde_json::Value::as_object)
        .expect("singular result contains one object");
    assert_eq!(singular.len(), 2);
    assert!(singular.contains_key("id") && singular.contains_key("kind"));

    let signals = execute_json(&pool, generated.get("signals").expect("signals SQL exists")).await;
    let signals = signals
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(serde_json::Value::as_object)
        .expect("signals result contains one object");
    assert_eq!(signals.get("rating"), Some(&serde_json::json!("4.0")));
    assert_eq!(signals.get("votes"), Some(&serde_json::json!("53")));

    let ratings = execute_json(&pool, generated.get("ratings").expect("ratings SQL exists")).await;
    assert_string_desc_id_asc(
        ratings.as_array().expect("ratings result is an array"),
        "info",
    );

    let ordered_title = execute_json(
        &pool,
        generated
            .get("ordered_title")
            .expect("ordered title SQL exists"),
    )
    .await;
    let aliases = ordered_title
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|title| title.get("aliases"))
        .and_then(serde_json::Value::as_array)
        .expect("ordered title contains aliases");
    assert_number_desc_id_asc(aliases, "kind_id");
}

#[tokio::test]
async fn scalar_aggregate_predicates_execute_empty_and_nonempty_relations_when_database_url_is_set()
{
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reference database connects");
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "aggregate-predicate-live.dsql",
        concat!(
            "query AggregatePredicateLive {\n",
            "  empty: title(where (.movie_info_idx | count) == 0 order by id asc limit 1) { id }\n",
            "  null_filtered: title(\n",
            "    where (.movie_info_idx | count) == 0\n",
            "      and (.movie_info_idx | min .info) != \"never\"\n",
            "    order by id asc\n",
            "    limit 1\n",
            "  ) { id }\n",
            "  with_exists: title(where .movie_info_idx | exists order by id asc limit 1) { id }\n",
            "  with_count: title(where (.movie_info_idx | count) > 0 order by id asc limit 1) { id }\n",
            "  numeric: title(\n",
            "    where (.movie_info_idx | sum .info_type_id) > 0\n",
            "      and (.movie_info_idx | avg .info_type_id) > 0\n",
            "    order by id asc\n",
            "    limit 1\n",
            "  ) { id }\n",
            "}\n",
        ),
    )
    .await;
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let generated = rows
        .collect()
        .into_iter()
        .map(|(_, fact)| (fact.0.output_name.clone(), fact.0.sql.clone()))
        .collect::<HashMap<_, _>>();
    let fetch_id = |name: &str| {
        generated
            .get(name)
            .cloned()
            .expect("aggregate predicate root generates SQL")
    };

    let empty = sqlx::query(AssertSqlSafe(fetch_id("empty")))
        .fetch_one(&pool)
        .await
        .expect("empty-relation count query executes");
    let null_filtered = sqlx::query(AssertSqlSafe(fetch_id("null_filtered")))
        .fetch_one(&pool)
        .await
        .expect("empty-relation nullable aggregate query executes");
    let with_exists = sqlx::query(AssertSqlSafe(fetch_id("with_exists")))
        .fetch_one(&pool)
        .await
        .expect("exists aggregate query returns a row");
    let with_count = sqlx::query(AssertSqlSafe(fetch_id("with_count")))
        .fetch_one(&pool)
        .await
        .expect("count aggregate query returns a row");
    let numeric = sqlx::query(AssertSqlSafe(fetch_id("numeric")))
        .fetch_one(&pool)
        .await
        .expect("numeric aggregate predicates return a row");

    let empty_result: serde_json::Value = empty
        .try_get("empty")
        .expect("empty result decodes as JSON");
    let null_filtered_result: serde_json::Value = null_filtered
        .try_get("null_filtered")
        .expect("null-filtered result decodes as JSON");
    assert_eq!(
        empty_result.as_array().map(Vec::len),
        Some(1),
        "count returns zero for an empty relation"
    );
    assert_eq!(
        null_filtered_result.as_array().map(Vec::len),
        Some(0),
        "comparison with min over an empty relation remains SQL unknown"
    );
    let exists_id: serde_json::Value = with_exists
        .try_get("with_exists")
        .expect("exists result decodes as JSON");
    let count_id: serde_json::Value = with_count
        .try_get("with_count")
        .expect("count result decodes as JSON");
    assert_eq!(
        exists_id, count_id,
        "exists and count select the same first row"
    );
    let _: serde_json::Value = numeric
        .try_get("numeric")
        .expect("numeric predicate result decodes as JSON");
}

fn assert_string_desc_id_asc(rows: &[serde_json::Value], primary: &str) {
    assert_eq!(rows.len(), 16, "limited collection keeps all rows");
    let mut saw_tie = false;
    for pair in rows.windows(2) {
        let left = pair[0]
            .get(primary)
            .and_then(serde_json::Value::as_str)
            .expect("ordered string field must be present");
        let right = pair[1]
            .get(primary)
            .and_then(serde_json::Value::as_str)
            .expect("ordered string field must be present");
        let left_id = pair[0]
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .expect("ordered id field must be present");
        let right_id = pair[1]
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .expect("ordered id field must be present");
        assert!(left >= right, "primary string order is descending");
        if left == right {
            saw_tie = true;
            assert!(left_id < right_id, "tied ids are ascending");
        }
    }
    assert!(saw_tie, "fixture must exercise secondary ordering");
}

fn assert_number_desc_id_asc(rows: &[serde_json::Value], primary: &str) {
    assert_eq!(rows.len(), 16, "limited collection keeps all rows");
    let mut saw_tie = false;
    for pair in rows.windows(2) {
        let left = pair[0]
            .get(primary)
            .and_then(serde_json::Value::as_i64)
            .expect("ordered numeric field must be present");
        let right = pair[1]
            .get(primary)
            .and_then(serde_json::Value::as_i64)
            .expect("ordered numeric field must be present");
        let left_id = pair[0]
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .expect("ordered id field must be present");
        let right_id = pair[1]
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .expect("ordered id field must be present");
        assert!(left >= right, "primary numeric order is descending");
        if left == right {
            saw_tie = true;
            assert!(left_id < right_id, "tied ids are ascending");
        }
    }
    assert!(saw_tie, "fixture must exercise secondary ordering");
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

#[tokio::test]
async fn grouped_aggregates_execute_empty_and_null_groups_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reference database connects");
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "grouped-live.dsql",
        concat!(
            "query GroupedLive {\n",
            "  empty: title(where .id != .id) | aggregate by .episode_nr { count }\n",
            "  episodes: title | aggregate by .episode_nr { count }\n",
            "}\n",
        ),
    )
    .await;
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let generated = rows
        .collect()
        .into_iter()
        .map(|(_, fact)| (fact.0.output_name.clone(), fact.0.sql.clone()))
        .collect::<HashMap<_, _>>();
    let empty = execute_json(
        &pool,
        generated.get("empty").expect("empty grouped SQL exists"),
    )
    .await;
    assert_eq!(empty, serde_json::json!([]), "empty grouped source is []");
    let episodes = execute_json(
        &pool,
        generated
            .get("episodes")
            .expect("episode grouped SQL exists"),
    )
    .await;
    let groups = episodes.as_array().expect("grouped result is an array");
    assert!(
        groups.iter().any(|group| group
            .get("episode_nr")
            .is_some_and(serde_json::Value::is_null)),
        "nullable key retains its NULL group"
    );
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
