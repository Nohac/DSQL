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

use crate::{fixture, fixture_names, imdb_catalog, numeric_catalog, set_source_text};

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
async fn predicate_extensions_render_with_postgres_semantics() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "predicate-extensions.dsql",
        concat!(
            "query PredicateExtensions {\n",
            "  title(\n",
            "    where not $$disabled\n",
            "      or .id in $$ids\n",
            "      and .production_year not in [1956, null]\n",
            "      and .episode_nr is null\n",
            "      and exists .movie_info_idx(where .info_type_id == ..kind_id)\n",
            "      and exists .movie_info_idx(where .info_type.info == .info)\n",
            "      and exists .movie_info_idx(where .info_type.id == ..kind_id)\n",
            "      and exists public::info_type(where .id == ..kind_id)\n",
            "      and exists public::info_type(\n",
            "        where exists public::kind_type(where .id == ..id)\n",
            "      )\n",
            "  ) { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn row_policies_apply_to_roots_relations_aggregates_and_predicate_sources() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "row-policies.dsql",
        concat!(
            "condition PreventTitleBypass { where not $:can_bypass_titles }\n",
            "filter TitleRows on title {\n",
            "  apply where PreventTitleBypass\n",
            "  where .kind_id == $:kind_id\n",
            "    and exists .movie_info_idx(where .info_type_id == ..kind_id)\n",
            "}\n",
            "filter InfoRows on movie_info_idx {\n",
            "  apply\n",
            "  where .info_type_id == 101\n",
            "}\n",
            "filter PositiveInfo on movie_info_idx { where .id > 0 }\n",
            "query DefaultRows {\n",
            "  title(limit 1) {\n",
            "    id\n",
            "    movie_info_idx { id }\n",
            "    info_count: movie_info_idx | aggregate { count }\n",
            "  }\n",
            "}\n",
            "query ConditionalBypass(filter InfoRows when false) {\n",
            "  title(filter TitleRows when false where exists .movie_info_idx) { id }\n",
            "}\n",
            "query ManualRows(filter PositiveInfo when $$positive_only) {\n",
            "  movie_info_idx { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn row_filters_execute_when_database_url_is_set() {
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
        "row-policy-live.dsql",
        concat!(
            "filter MinimumTitle on title {\n",
            "  apply\n",
            "  where .id > $:minimum_id\n",
            "}\n",
            "filter ManualTitle on title { where .id > 1 }\n",
            "query Filtered(filter ManualTitle when $$enabled) {\n",
            "  title(order by id asc limit 1) { id }\n",
            "}\n",
        ),
    )
    .await;
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let generated = rows
        .collect()
        .into_iter()
        .map(|(_, fact)| &fact.0)
        .next()
        .expect("row-filtered query generates SQL");
    let parameter_paths = generated
        .parameters
        .iter()
        .map(|parameter| parameter.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        parameter_paths,
        ["context.minimum_id", "params.enabled"],
        "trusted context and public input retain distinct parameter provenance",
    );
    let sql = format!(
        "select ({})::text",
        generated.sql.trim().trim_end_matches(';')
    );

    for (minimum_id, enabled, expected_id) in [(0_i32, false, 1), (1, false, 2), (0, true, 2)] {
        let row = sqlx::query(AssertSqlSafe(sql.clone()))
            .bind(minimum_id)
            .bind(enabled)
            .fetch_one(&pool)
            .await
            .expect("row-filtered SQL executes");
        let json: String = row
            .try_get::<Option<String>, _>(0)
            .expect("query returns nullable JSON text")
            .expect("fixture query returns one title");
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("generated output is JSON");
        assert_eq!(value.get("id"), Some(&serde_json::json!(expected_id)));
    }
}

#[tokio::test]
async fn field_filters_mask_every_query_authored_read_and_relation_traversal() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "field-policies.dsql",
        concat!(
            "condition CanReadTitle { where $:can_read_title }\n",
            "filter TitlePrivacy on title {\n",
            "  apply\n",
            "  field title, movie_info_idx where CanReadTitle\n",
            "}\n",
            "filter InfoPrivacy on movie_info_idx {\n",
            "  apply\n",
            "  field info where $:can_read_info\n",
            "}\n",
            "filter Unused on title {\n",
            "  apply\n",
            "  field production_year where $:unused\n",
            "}\n",
            "query MaskedReads {\n",
            "  title(\n",
            "    where .title == $$guess\n",
            "      and .movie_info_idx.info in $$infos\n",
            "      and exists .movie_info_idx(where .info == $$info_guess)\n",
            "      and (.movie_info_idx | count .info) > 0\n",
            "    order by title $$direction\n",
            "    limit 1\n",
            "  ) {\n",
            "    id\n",
            "    title\n",
            "    movie_info_idx { id info }\n",
            "    info_stats: movie_info_idx | aggregate {\n",
            "      count\n",
            "      visible: count .info\n",
            "      first: min .info\n",
            "    }\n",
            "  }\n",
            "  grouped: title | aggregate by title_group: .title { count }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn predicate_keywords_remain_valid_catalog_identifiers() {
    let bowl = sql_bowl(numeric_catalog()).await;
    insert_source(
        &bowl,
        "contextual-identifiers.dsql",
        concat!(
            "fragment not on metrics { exists }\n",
            "query exists {\n",
            "  metrics(\n",
            "    where .exists == 1 and .in == 2 and .is == 3 and .not == 4\n",
            "    order by exists asc\n",
            "  ) {\n",
            "    ...not\n",
            "    in: exists\n",
            "    is\n",
            "    not\n",
            "  }\n",
            "}\n",
        ),
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
async fn field_filters_enforce_readable_views_when_database_url_is_set() {
    let Ok(database_url) = env::var("DSQL_TEST_DATABASE_URL") else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reference database connects");
    let reference = sqlx::query(concat!(
        "select title.id, title.title from public.title ",
        "where title.id > 0 and title.title is not null ",
        "and exists (select 1 from public.movie_info_idx ",
        "where movie_info_idx.movie_id = title.id) ",
        "order by title.id limit 1",
    ))
    .fetch_one(&pool)
    .await
    .expect("reference title with related rows exists");
    let reference_id: i32 = reference.try_get(0).expect("reference id is int4");
    let reference_title: String = reference.try_get(1).expect("reference title is text");
    let unknown = sqlx::query(concat!(
        "select id from public.title where id > 0 ",
        "and title is not null and production_year is null order by id limit 1",
    ))
    .fetch_one(&pool)
    .await
    .expect("reference title with unknown policy condition exists");
    let unknown_id: i32 = unknown.try_get(0).expect("unknown-condition id is int4");

    let bowl = sql_bowl_with_limit(imdb_catalog(), None).await;
    insert_source(
        &bowl,
        "field-policy-live.dsql",
        &format!(
            concat!(
                "filter PositiveRows on title {{ apply where .id > 0 }}\n",
                "filter TitlePrivacy on title {{\n",
                "  apply\n",
                "  field title, movie_info_idx where $:can_read_title\n",
                "}}\n",
                "filter UnknownTitle on title {{ field title where .production_year > 0 }}\n",
                "filter HideId on title {{ field id where false }}\n",
                "query Probe {{\n",
                "  probe: title(where .id == {reference_id} and .title == $$guess limit 1) {{ id }}\n",
                "}}\n",
                "query Membership {{\n",
                "  membership: title(where .id == {reference_id} and .title in $$titles limit 1) {{ id }}\n",
                "}}\n",
                "query Relation {{\n",
                "  relation: title(where .id == {reference_id} limit 1) {{\n",
                "    title\n",
                "    movie_info_idx {{ id }}\n",
                "    info_stats: movie_info_idx | aggregate {{ count }}\n",
                "  }}\n",
                "}}\n",
                "query Exists {{\n",
                "  related: title(where .id == {reference_id} and exists .movie_info_idx limit 1) {{ id }}\n",
                "}}\n",
                "query Grouped {{\n",
                "  grouped: title | aggregate by .title {{ count }}\n",
                "}}\n",
                "query Unknown(filter UnknownTitle) {{\n",
                "  unknown: title(where .id == {unknown_id} limit 1) {{ title }}\n",
                "}}\n",
                "query RawBoundary(filter HideId) {{ raw: title(limit 1) {{ production_year }} }}\n",
            ),
            reference_id = reference_id,
            unknown_id = unknown_id,
        ),
    )
    .await;
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let generated = rows
        .collect()
        .into_iter()
        .map(|(_, fact)| {
            (
                fact.0.output_name.clone(),
                (
                    fact.0.sql.clone(),
                    fact.0
                        .parameters
                        .iter()
                        .map(|parameter| parameter.path.clone())
                        .collect::<Vec<_>>(),
                ),
            )
        })
        .collect::<HashMap<_, _>>();

    let (probe_sql, probe_parameters) = generated.get("probe").expect("probe SQL exists");
    assert_eq!(
        probe_parameters,
        &["context.can_read_title", "params.guess"]
    );
    let probe_sql = format!("select ({})::text", probe_sql.trim().trim_end_matches(';'));
    let visible = sqlx::query(AssertSqlSafe(probe_sql.clone()))
        .bind(true)
        .bind(&reference_title)
        .fetch_one(&pool)
        .await
        .expect("visible exact-match probe executes");
    assert!(
        visible
            .try_get::<Option<String>, _>(0)
            .expect("visible probe returns JSON text")
            .is_some(),
        "an authorized exact match returns the row",
    );
    for guess in [Some(reference_title.as_str()), None] {
        let hidden = sqlx::query(AssertSqlSafe(probe_sql.clone()))
            .bind(false)
            .bind(guess)
            .fetch_one(&pool)
            .await
            .expect("hidden probe executes");
        assert!(
            hidden
                .try_get::<Option<String>, _>(0)
                .expect("hidden probe returns nullable JSON text")
                .is_none(),
            "a hidden value cannot be discovered by equality or NULL probes",
        );
    }

    let (membership_sql, membership_parameters) =
        generated.get("membership").expect("membership SQL exists");
    assert_eq!(
        membership_parameters,
        &["context.can_read_title", "params.titles"]
    );
    let membership = sqlx::query(AssertSqlSafe(format!(
        "select ({})::text",
        membership_sql.trim().trim_end_matches(';')
    )))
    .bind(false)
    .bind(vec![reference_title])
    .fetch_one(&pool)
    .await
    .expect("hidden membership probe executes");
    assert!(
        membership
            .try_get::<Option<String>, _>(0)
            .expect("membership returns nullable JSON text")
            .is_none(),
        "membership cannot probe a hidden value",
    );

    let (relation_sql, relation_parameters) = generated
        .get("relation")
        .expect("relation projection SQL exists");
    assert_eq!(relation_parameters, &["context.can_read_title"]);
    let relation_sql = format!(
        "select ({})::text",
        relation_sql.trim().trim_end_matches(';')
    );
    let hidden_relation = sqlx::query(AssertSqlSafe(relation_sql.clone()))
        .bind(false)
        .fetch_one(&pool)
        .await
        .expect("hidden relation query executes");
    let hidden_relation: String = hidden_relation
        .try_get::<Option<String>, _>(0)
        .expect("relation returns nullable JSON text")
        .expect("the parent row remains visible");
    let hidden_relation: serde_json::Value =
        serde_json::from_str(&hidden_relation).expect("relation result is JSON");
    assert!(hidden_relation["title"].is_null());
    assert_eq!(hidden_relation["movie_info_idx"], serde_json::json!([]));
    assert_eq!(hidden_relation["info_stats"]["count"], serde_json::json!(0));
    let visible_relation = sqlx::query(AssertSqlSafe(relation_sql))
        .bind(true)
        .fetch_one(&pool)
        .await
        .expect("visible relation query executes");
    let visible_relation: String = visible_relation
        .try_get::<Option<String>, _>(0)
        .expect("visible relation returns nullable JSON text")
        .expect("the visible parent row exists");
    let visible_relation: serde_json::Value =
        serde_json::from_str(&visible_relation).expect("visible relation result is JSON");
    assert!(
        visible_relation["movie_info_idx"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "authorized relation traversal retains related rows",
    );

    let (related_sql, related_parameters) = generated.get("related").expect("exists SQL exists");
    assert_eq!(related_parameters, &["context.can_read_title"]);
    let related = sqlx::query(AssertSqlSafe(format!(
        "select ({})::text",
        related_sql.trim().trim_end_matches(';')
    )))
    .bind(false)
    .fetch_one(&pool)
    .await
    .expect("hidden exists probe executes");
    assert!(
        related
            .try_get::<Option<String>, _>(0)
            .expect("exists probe returns nullable JSON text")
            .is_none(),
        "a masked relation behaves as empty in exists",
    );

    let (grouped_sql, grouped_parameters) = generated.get("grouped").expect("grouped SQL exists");
    assert_eq!(grouped_parameters, &["context.can_read_title"]);
    let grouped = sqlx::query(AssertSqlSafe(format!(
        "select ({})::text",
        grouped_sql.trim().trim_end_matches(';')
    )))
    .bind(false)
    .fetch_one(&pool)
    .await
    .expect("masked grouped query executes");
    let grouped: String = grouped.try_get(0).expect("grouped query returns JSON text");
    let grouped: serde_json::Value =
        serde_json::from_str(&grouped).expect("grouped result is JSON");
    let groups = grouped.as_array().expect("grouped result is an array");
    assert_eq!(
        groups.len(),
        1,
        "hidden values collapse into one NULL group"
    );
    assert!(groups[0]["title"].is_null());

    let (unknown_sql, unknown_parameters) =
        generated.get("unknown").expect("unknown guard SQL exists");
    assert_eq!(unknown_parameters, &["context.can_read_title"]);
    let unknown = sqlx::query(AssertSqlSafe(format!(
        "select ({})::text",
        unknown_sql.trim().trim_end_matches(';')
    )))
    .bind(true)
    .fetch_one(&pool)
    .await
    .expect("unknown-condition query executes");
    let unknown: String = unknown
        .try_get::<Option<String>, _>(0)
        .expect("unknown-condition query returns nullable JSON text")
        .expect("unknown-condition parent row exists");
    let unknown: serde_json::Value =
        serde_json::from_str(&unknown).expect("unknown-condition result is JSON");
    assert!(
        unknown["title"].is_null(),
        "an unknown field guard masks the value",
    );

    let (raw_sql, raw_parameters) = generated.get("raw").expect("raw-boundary SQL exists");
    assert!(
        raw_parameters.is_empty(),
        "an unused mask does not add a context parameter",
    );
    assert!(
        execute_json(&pool, raw_sql).await.is_object(),
        "row policy conditions read raw fields even when query reads mask them",
    );
}

#[tokio::test]
async fn null_comparisons_render_postgres_null_predicates() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "null-predicates.dsql",
        concat!(
            "query NullPredicates {\n",
            "  title(\n",
            "    where .production_year == null\n",
            "      or null != .kind_id\n",
            "      or .production_year $$null_operator[==, !=] null\n",
            "    limit 2\n",
            "  ) { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn singular_roots_and_relations_render_nullable_object_envelopes() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "singular-shapes.dsql",
        concat!(
            "query SingularShapes {\n",
            "  by_limit: title(order by id asc limit 1) { id title }\n",
            "  by_key: title(where .id == $$id) {\n",
            "    id\n",
            "    latest_info: movie_info(order by id desc limit 1) { id info }\n",
            "  }\n",
            "  runtime: title(limit $$count) { id }\n",
            "  ...title(where .id == $$flat_id) { flat_id: id flat_title: title }\n",
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
async fn policy_body_edits_rederive_dependent_sql() {
    let bowl = sql_bowl(imdb_catalog()).await;
    let policy = insert_source(
        &bowl,
        "policy.dsql",
        "filter PositiveTitles on title { apply where true where .id > 0 }\n",
    )
    .await;
    insert_source(
        &bowl,
        "query.dsql",
        "query Titles { title(limit 1) { id } }\n",
    )
    .await;
    let before = render_sql(&bowl).await;

    set_source_text(
        &bowl,
        policy,
        "filter PositiveTitles on title { apply where true where .id > 1 }\n",
    )
    .await;
    let after = render_sql(&bowl).await;

    insta::assert_snapshot!(format!("before:\n{before}\n\nafter:\n{after}"));
}

#[tokio::test]
async fn policy_match_edits_rederive_fragment_dependent_sql() {
    let bowl = sql_bowl(imdb_catalog()).await;
    let policy = insert_source(
        &bowl,
        "policy.dsql",
        "filter Moving on { .kind: text } { apply where true where .id > 900001 }\n",
    )
    .await;
    insert_source(
        &bowl,
        "query.dsql",
        concat!(
            "fragment Nested on title {\n",
            "  kind_type { id kind }\n",
            "  movie_info_idx { id info }\n",
            "}\n",
            "query Q { title(limit 1) { ...Nested } }\n",
        ),
    )
    .await;
    let before = render_sql(&bowl).await;

    set_source_text(
        &bowl,
        policy,
        "filter Moving on { .info: text } { apply where true where .id > 900001 }\n",
    )
    .await;
    let after = render_sql(&bowl).await;

    insta::assert_snapshot!(format!("before:\n{before}\n\nafter:\n{after}"));
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
    insert_source(
        &bowl,
        "missing-singular.dsql",
        concat!(
            "query MissingSingular {\n",
            "  missing: title(where .id == -1) { id }\n",
            "  ...title(where .id == -1) { missing_id: id }\n",
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
    let singular = singular.as_object().expect("singular result is an object");
    assert_eq!(singular.len(), 2);
    assert!(singular.contains_key("id") && singular.contains_key("kind"));

    let signals = execute_json(&pool, generated.get("signals").expect("signals SQL exists")).await;
    let signals = signals.as_object().expect("signals result is an object");
    assert_eq!(signals.get("rating"), Some(&serde_json::json!("4.0")));
    assert_eq!(signals.get("votes"), Some(&serde_json::json!("53")));

    let missing = execute_json(&pool, generated.get("missing").expect("missing SQL exists")).await;
    assert!(missing.is_null(), "an absent singular root is null");
    let missing_flattened = sqlx::query(AssertSqlSafe(
        generated
            .get("title")
            .expect("missing flattened SQL exists")
            .clone(),
    ))
    .fetch_one(&pool)
    .await
    .expect("missing flattened SQL keeps its protocol row");
    let missing_id: Option<serde_json::Value> = missing_flattened
        .try_get("missing_id")
        .expect("missing flattened field decodes as JSON null");
    assert!(
        missing_id.is_none(),
        "an absent flattened root field is null"
    );

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
        .get("aliases")
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
        insert_source(&bowl, name, &fixture(name)).await;
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
                value.is_array() || value.is_object() || value.is_null(),
                "{name} generated JSON must match a collection or singular result"
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
        insert_source(&bowl, name, &fixture(name)).await;
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
    let json: Option<String> = row.try_get(0).expect("query returns nullable JSON text");
    json.map_or(serde_json::Value::Null, |json| {
        serde_json::from_str(&json).expect("generated output is JSON")
    })
}
