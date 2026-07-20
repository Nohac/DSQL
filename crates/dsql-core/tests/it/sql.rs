//! Plan and SQL generation: fixture queries plan and render to PostgreSQL
//! on demand.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{PlanDemand, SqlDemand};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};

use crate::{fixture, imdb_catalog, numeric_catalog, set_source_text};

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
            "      and (.title is null or .title is not null)\n",
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
