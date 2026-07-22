//! Plan and SQL generation: fixture queries plan and render to PostgreSQL
//! on demand.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{DiagnosticsDemand, PlanDemand, SqlDemand, VariablesDemand};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};

use crate::{fixture, imdb_catalog, numeric_catalog, render_diagnostic_facts, set_source_text};

async fn sql_bowl(catalog: Catalog) -> Bowl {
    sql_bowl_with_limit(catalog, Some(10)).await
}

async fn sql_bowl_with_limit(catalog: Catalog, collection_limit: Option<u64>) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
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

const MOVIE_SIGNALS: &str = indoc::indoc! {r#"
    fragment MovieSignals on title {
      ...movie_info_idx(where .info_type_id == 101) | aggregate {
        rating: max .info
      }
      ...movie_info_idx(where .info_type_id == 100) | aggregate {
        votes: max .info
      }
    }
"#};

const RENDERER_EDGE_QUERY: &str = indoc::indoc! {r#"
    query RendererEdges {
      signals: title(where .id == 2 limit 1) { id ...MovieSignals }
      ratings: movie_info_idx(
        where .info_type_id == 101
        order by info desc, id asc
        limit 16
      ) { id info }
      ordered_title: title(where .id == 943844 limit 1) {
        aliases: aka_title->movie_id(order by kind_id desc, id asc limit 16) {
          id
          kind_id
        }
      }
      singular: title(where .id == 2 limit 1) {
        id
        ...kind_type { kind }
      }
      ...kind_type | aggregate {
        kind_count: count
        first_kind: min .kind
      }
    }
"#};

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
async fn nullable_inputs_prune_predicates_clauses_ordering_and_cardinality() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "nullable-inputs.dsql",
        indoc::indoc! {r#"
            query NullableInputs(
              $$id? = null
              $$from? = null
              $$to? = null
              $$direction? = null
              $$limit? = null
            ) {
              public::users(
                where .id == $$id and (.id >= $$from or not (.id <= $$to))
                order by id $$direction
                limit $$limit
              ) { id }
            }
            query DefaultedUnique($$id = 1) {
              public::users(where .id == $$id) { id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn nullable_predicate_operands_wrap_complete_atoms_in_every_order() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insert_source(
        &bowl,
        "nullable-predicate-operands.dsql",
        indoc::indoc! {r#"
            query OptionalAtoms(
              $$id? = null
              $$left? = null
              $$right? = null
            ) {
              direct: public::users(where .id == $$id) { id }
              seed: public::users(where .id == $$left and .id == $$right) { id }
              reversed: public::users(
                where $$id == .id or .name == "fallback"
              ) { id }
              multiple: public::users(
                where $$left == $$right or .name == "fallback"
              ) { id }
              negated: public::users(where not ($$id == .id)) { id }
            }
        "#},
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn valid_collection_and_pagination_defaults_survive_every_binding_shape() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insert_source(
        &bowl,
        "default-binding-shapes.dsql",
        indoc::indoc! {r#"
            fragment DefaultWindow(
              $$ids = ["00000000-0000-0000-0000-000000000001"]
              $$limit = 5
              $$offset = 2
            ) on public::users {
              posts(where .id in $$ids limit $$limit offset $$offset) { id }
            }
            query DirectDefaults(
              $$ids? = null
              $$limit = 0
              $$offset = 2
            ) {
              public::posts(
                where .id in $$ids
                limit $$limit
                offset $$offset
              ) { id }
            }
            query ContainedDefaults {
              public::users { ...DefaultWindow }
            }
            query LiftedDefaults {
              public::users { ...DefaultWindow($$) }
            }
            query OmittedDefaults($$limit = 7) {
              public::users { ...DefaultWindow($$limit) }
            }
        "#},
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn fragment_bindings_rewrite_sql_inputs_and_inline_omitted_defaults() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "fragment-bindings.dsql",
        indoc::indoc! {r#"
            fragment PostWindow($$minimum = "M" $$limit = 5) on public::users {
              posts(where .title >= $$minimum limit $$) { id }
            }
            fragment ParentWindow on public::users {
              ...PostWindow($$)
            }
            query Contained { public::users { ...PostWindow } }
            query Bound($$outer = "N") {
              public::users { ...PostWindow($$minimum <- $$outer) }
            }
            query Namespaced {
              public::users { ...PostWindow($$ <- $$window) }
            }
            query Nested($$outer = "N") {
              public::users { ...ParentWindow($$minimum <- $$outer) }
            }
        "#},
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
        indoc::indoc! {r#"
            query PredicateExtensions {
              title(
                where not $$disabled
                  or .id in $$ids
                  and .production_year not in [1956, null]
                  and .episode_nr is null
                  and exists .movie_info_idx(where .info_type_id == ..kind_id)
                  and exists .movie_info_idx(where .info_type.info == .info)
                  and exists .movie_info_idx(where .info_type.id == ..kind_id)
                  and exists public::info_type(where .id == ..kind_id)
                  and exists public::info_type(
                    where exists public::kind_type(where .id == ..id)
                  )
              ) { id }
            }
        "#},
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
        indoc::indoc! {r#"
            condition PreventTitleBypass { where not $:can_bypass_titles }
            filter TitleRows on title {
              apply where PreventTitleBypass
              where .kind_id == $:kind_id
                and exists .movie_info_idx(where .info_type_id == ..kind_id)
            }
            filter InfoRows on movie_info_idx {
              apply
              where .info_type_id == 101
            }
            filter PositiveInfo on movie_info_idx { where .id > 0 }
            query DefaultRows {
              title(limit 1) {
                id
                movie_info_idx { id }
                info_count: movie_info_idx | aggregate { count }
              }
            }
            query ConditionalBypass(filter InfoRows when false) {
              title(filter TitleRows when false where exists .movie_info_idx) { id }
            }
            query ManualRows(filter PositiveInfo when $$positive_only) {
              movie_info_idx { id }
            }
        "#},
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
        indoc::indoc! {r#"
            condition CanReadTitle { where $:can_read_title }
            filter TitlePrivacy on title {
              apply
              field title, movie_info_idx where CanReadTitle
            }
            filter InfoPrivacy on movie_info_idx {
              apply
              field info where $:can_read_info
            }
            filter Unused on title {
              apply
              field production_year where $:unused
            }
            query MaskedReads {
              title(
                where .title == $$guess
                  and (.title is null or .title is not null)
                  and .movie_info_idx.info in $$infos
                  and exists .movie_info_idx(where .info == $$info_guess)
                  and (.movie_info_idx | count .info) > 0
                order by title $$direction
                limit 1
              ) {
                id
                title
                movie_info_idx { id info }
                info_stats: movie_info_idx | aggregate {
                  count
                  visible: count .info
                  first: min .info
                }
              }
              grouped: title | aggregate by title_group: .title { count }
            }
        "#},
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
        indoc::indoc! {r#"
            fragment not on metrics { exists }
            query exists {
              metrics(
                where .exists == 1 and .in == 2 and .is == 3 and .not == 4
                order by exists asc
              ) {
                ...not
                in: exists
                is
                not
              }
            }
        "#},
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
        indoc::indoc! {r#"
            query NumericMetrics {
              metrics(where .amount >= 12345678901234567890.12345678901234567890) {
                amount
                ratio
              }
              groups: metrics | aggregate by amount_group: .amount, ratio_group: .ratio {
                count
                total_amount: sum .amount
                average_ratio: avg .ratio
              }
              summary: metrics | aggregate {
                total_amount: sum .amount
                average_amount: avg .amount
                total_ratio: sum .ratio
                average_ratio: avg .ratio
              }
            }
        "#},
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
        indoc::indoc! {r#"
            query NumericTemplate {
              metrics(
                where .amount == 9000000000000000000
                  or .amount == 9000000000000000005.5
                  or .amount == 12345678901234567890.12345678901234567890
                limit $$page_limit
                offset $$page_offset
              ) { amount }
            }
        "#},
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
        indoc::indoc! {r#"
            query AggregateSql {
              stats: public::users(where .email != null) | aggregate {
                count
                populated_email: count .email
                any: exists
                first_name: min .name
                latest_signup: max .created_at
              }
              by_email: public::users | aggregate by email_group: .email {
                count
                earliest_signup: min .created_at
              }
              public::users(limit 2) {
                id
                post_stats: posts(where .title like "%x%") | aggregate {
                  count
                  latest: max .created_at
                }
                post_groups: posts | aggregate by title_group: .title {
                  count
                  latest: max .created_at
                }
              }
            }
        "#},
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
        indoc::indoc! {r#"
            query AggregatePredicateSql {
              title(
                where .movie_info_idx | exists
                  and .movie_info_idx | count >= $minimum
                  and $maximum >= (.movie_info_idx | count)
                  and 0 < (.movie_info_idx | count .info)
                  and (.movie_info_idx | count .info) > 0
                  and (.movie_info_idx | count .info) <= (.movie_info_idx | count)
                  and (.movie_info_idx | min .info) like "4.%"
                  and (.movie_info_idx | max .info) != null
                  and (.movie_info_idx | sum .info_type_id) > 0
                  and (.movie_info_idx | avg .info_type_id) > 0
                order by id asc
                limit 5
              ) { id }
            }
        "#},
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
        indoc::indoc! {r#"
            query NullPredicates {
              title(
                where .production_year == null
                  or null != .kind_id
                  or .production_year $$null_operator[==, !=] null
                limit 2
              ) { id }
            }
        "#},
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
        indoc::indoc! {r#"
            query SingularShapes {
              by_limit: title(order by id asc limit 1) { id title }
              by_key: title(where .id == $$id) {
                id
                latest_info: movie_info(order by id desc limit 1) { id info }
              }
              runtime: title(limit $$count) { id }
              ...title(where .id == $$flat_id) { flat_id: id flat_title: title }
            }
        "#},
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
        indoc::indoc! {r#"
            query FlattenOwner {
              feed: public::posts(limit 1) {
                id
                ...users(where .name like $$owner) {
                  owner_name: name
                  recent: posts(limit 1) { title }
                }
              }
            }
            query FlattenPostStats {
              accounts: public::users(limit 1) {
                id
                ...posts(where .title like $$title) | aggregate {
                  post_count: count
                  latest_post: max .created_at
                }
              }
            }
            query FlattenRoot {
              ...public::users(where .name == $$root_name) | aggregate {
                user_count: count
                first_name: min .name
              }
            }
        "#},
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
        indoc::indoc! {r#"
            fragment Nested on title {
              kind_type { id kind }
              movie_info_idx { id info }
            }
            query Q { title(limit 1) { ...Nested } }
        "#},
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
        indoc::indoc! {r#"
            fragment RatingFields on title {
              ratings: movie_info_idx(where .info_type_id == 101 order by id asc limit 1) {
                info
              }
            }
        "#},
    )
    .await;
    insert_source(
        &bowl,
        "top-rated.dsql",
        indoc::indoc! {r#"
            query TopRated {
              title(limit 1) {
                id
                ...RatingFields
              }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}
