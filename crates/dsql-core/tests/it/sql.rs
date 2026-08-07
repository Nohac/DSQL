//! Plan and SQL generation: fixture queries plan and render to PostgreSQL
//! on demand.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::variable::DefinitionVariables;
use dsql_core::facts::{DefKey, DiagnosticsDemand, PlanDemand, SqlDemand, VariablesDemand};
use dsql_core::language_bowl;
use dsql_core::plan::QueryPlanFact;
use dsql_core::source::{insert_embedding_source, insert_source};
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};

use crate::{
    fixture, imdb_catalog, numeric_catalog, provider_scalar_catalog, render_diagnostic_facts,
    set_source_text, structured_type_catalog,
};

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
    generated.sort_by(|left, right| left.0.operation_name.cmp(&right.0.operation_name));
    generated
        .into_iter()
        .map(|fact| {
            let mut section = format!("-- {}\n{}", fact.0.operation_name, fact.0.sql);
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

async fn definition_product_ids(bowl: &Bowl, name: &str, stage: &str) -> (Entity, Entity) {
    let definitions = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let definition = definitions
        .collect()
        .into_iter()
        .find_map(|(entity, declaration)| (declaration.name == name).then_some(entity))
        .expect("named definition exists");
    let variables = bowl
        .scoop::<Query<(Entity, &DefinitionVariables, &DefKey)>>()
        .await
        .collect()
        .into_iter()
        .filter_map(|(entity, _, key)| (key.0 == definition).then_some(entity))
        .collect::<Vec<_>>();
    let plans = bowl
        .scoop::<Query<(Entity, &QueryPlanFact, &DefKey)>>()
        .await
        .collect()
        .into_iter()
        .filter_map(|(entity, _, key)| (key.0 == definition).then_some(entity))
        .collect::<Vec<_>>();
    assert_eq!(
        variables.len(),
        1,
        "one variable contract per definition at {stage}"
    );
    assert_eq!(plans.len(), 1, "one query plan per definition at {stage}");
    (variables[0], plans[0])
}

async fn render_sql_forms(bowl: &Bowl) -> String {
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let mut generated: Vec<&GeneratedSqlFact> =
        rows.collect().into_iter().map(|(_, fact)| fact).collect();
    generated.sort_by(|left, right| left.0.operation_name.cmp(&right.0.operation_name));
    generated
        .into_iter()
        .map(|fact| {
            format!(
                "-- formatted\n{}\n\n-- compact\n{}",
                fact.0.sql, fact.0.compact_sql
            )
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
            "query UsersPage {\n  public::users(\n    where .name %name_op[==, like] %name and .id == $tenant\n    order by created_at %dir\n    limit %max\n    offset $skip\n  ) {\n    id\n    posts(limit 5) {\n      title\n    }\n  }\n}\n",
        )
        .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn formatted_and_compact_sql_share_template_substitutions() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "output-modes.dsql",
        indoc::indoc! {r#"
            query OutputModes {
              public::users(
                where .id >= %minimum
                order by id %direction[asc, desc]
                limit %limit
              ) {
                id
                name
              }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_sql_forms(&bowl).await);
}

#[tokio::test]
async fn bounded_dynamic_markers_retry_on_generated_sql_collisions() {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "dynamic-marker-collision.dsql",
        indoc::indoc! {r#"
            query MarkerCollision(%search = {}) {
              title(
                where .title == "{{dynamic:0}}"
                  and %search on selected
                limit 1
              ) {
                id
                title
              }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_sql_forms(&bowl).await);
}

#[tokio::test]
async fn nullable_inputs_prune_predicates_clauses_ordering_and_cardinality() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "nullable-inputs.dsql",
        indoc::indoc! {r#"
            query NullableInputs(
              %id? = null
              %from? = null
              %to? = null
              %direction? = null
              %limit? = null
            ) {
              public::users(
                where .id == %id and (.id >= %from or not (.id <= %to))
                order by id %direction
                limit %limit
              ) { id }
            }
            query DefaultedUnique(%id = 1) {
              public::users(where .id == %id) { id }
            }
            query OptionalEdges(
              %ids? = null
              %offset? = null
              %enabled? = null
              %reversed? = null
            ) {
              public::users(
                where .id in %ids
                  or %enabled
                  or %reversed == .id
                offset %offset
              ) { id }
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
              %id? = null
              %left? = null
              %name? = null
              %required_name?
              %right? = null
            ) {
              direct: public::users(where .id == %id) { id }
              pattern: public::users(where .name like %name) { id }
              required_pattern: public::users(
                where .name like %required_name
              ) { id }
              seed: public::users(where .id == %left and .id == %right) { id }
              reversed: public::users(
                where %id == .id or .name == "fallback"
              ) { id }
              multiple: public::users(
                where %left == %right or .name == "fallback"
              ) { id }
              negated: public::users(where not (%id == .id)) { id }
            }
        "#},
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn featured_movie_optional_like_sql_is_typed_before_its_presence_guard() {
    let bowl = sql_bowl(imdb_catalog()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insert_source(
        &bowl,
        "hero-panel.dsql",
        indoc::indoc! {r#"
            fragment HeroPanelFields on title {
              id
              title
            }
        "#},
    )
    .await;
    insert_embedding_source(
        &bowl,
        "FeaturedMovie.tsx",
        indoc::indoc! {r#"
            export const FeaturedMovieQuery = dsql(`
            query FeaturedMovieQuery(
              %info?
            ) {
              featured: movie_info_idx(where .info_type_id == 101
                and .info like %
                and .title.kind_id == 1
                and .title.movie_info_idx.info_type_id == 100
                order by info desc, id asc limit 1
              ) {
                ...title {
                  ...HeroPanelFields
                }
              }
            }
            `);

            export function FeaturedMovie() {
              return useQuery(FeaturedMovieQuery);
            }
        "#},
        "typescript",
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn nested_optional_predicate_trees_keep_parameters_typed() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insert_source(
        &bowl,
        "nested-optional-predicates.dsql",
        indoc::indoc! {r#"
            query NestedOptionalPredicates(
              %user_name? = null
              %post_title? = null
              %author_name? = null
              %minimum_id? = null
              %maximum_id? = null
            ) {
              public::users(
                where (
                  .name like %user_name
                  or not (.id >= %minimum_id and .id <= %maximum_id)
                ) and (
                  .posts.title like %post_title
                  or .posts.users.name like %author_name
                )
              ) {
                id
                posts(
                  where (
                    .title like %post_title
                    and .users.name like %author_name
                  ) or not (
                    .users.id >= %minimum_id
                    and .users.id <= %maximum_id
                  )
                ) {
                  id
                  title
                }
              }
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
              %ids = ["00000000-0000-0000-0000-000000000001"]
              %limit = 5
              %offset = 2
            ) on public::users {
              posts(where .id in %ids limit %limit offset %offset) { id }
            }
            query DirectDefaults(
              %ids? = null
              %limit = 0
              %offset = 2
            ) {
              public::posts(
                where .id in %ids
                limit %limit
                offset %offset
              ) { id }
            }
            query ContainedDefaults {
              public::users { ...DefaultWindow }
            }
            query LiftedDefaults {
              public::users { ...DefaultWindow(%) }
            }
            query OmittedDefaults(%limit = 7) {
              public::users { ...DefaultWindow(%limit) }
            }
        "#},
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn invalid_pagination_is_fail_closed_when_planning_is_forced() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "invalid-pagination.dsql",
        indoc::indoc! {r#"
            query OffsetBeforeInvalidLimit {
              public::users(offset 5 limit -1) { id }
            }
            query InvalidLimitBeforeOffset {
              public::users(limit -1 offset 5) { id }
            }
            query ValidLimitBeforeInvalidOffset {
              public::users(limit 10 offset 9223372036854775808) { id }
            }
            query ValidBoundaries {
              zero: public::users(limit 0) { id }
              maximum: public::users(offset 9223372036854775807) { id }
            }
            query NestedIsolation {
              public::users(limit 5) {
                posts(limit -1) { id }
              }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_sql(&bowl).await);
}

#[tokio::test]
async fn fragment_bindings_rewrite_sql_inputs_and_inline_omitted_defaults() {
    let bowl = sql_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "fragment-bindings.dsql",
        indoc::indoc! {r#"
            fragment PostWindow(%minimum = "M" %limit = 5) on public::users {
              posts(where .title >= %minimum limit %) { id }
            }
            fragment ParentWindow on public::users {
              ...PostWindow(%)
            }
            query Contained { public::users { ...PostWindow } }
            query Bound(%outer = "N") {
              public::users { ...PostWindow(%minimum <- %outer) }
            }
            query Namespaced {
              public::users { ...PostWindow(% <- %window) }
            }
            query Nested(%outer = "N") {
              public::users { ...ParentWindow(%minimum <- %outer) }
            }
            query DeepContained {
              public::users { ...ParentWindow }
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
                where not %disabled
                  or .id in %ids
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
            context {
              can_bypass_titles: bool
              kind_id: int
            }
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
            query ManualRows(filter PositiveInfo when %positive_only) {
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
            context {
              can_read_title: bool
              can_read_info: bool
              unused: bool
            }
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
                where .title == %guess
                  and (.title is null or .title is not null)
                  and .movie_info_idx.info in %infos
                  and exists .movie_info_idx(where .info == %info_guess)
                  and (.movie_info_idx | count .info) > 0
                order by title %direction
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
async fn provider_scalars_use_schema_qualified_text_casts_everywhere() {
    let bowl = sql_bowl(provider_scalar_catalog()).await;
    insert_source(
        &bowl,
        "provider-scalars.dsql",
        indoc::indoc! {r#"
            context { trusted_address: pg_catalog::inet }
            filter AddressGate on events {
              apply where true
              where .address == $:trusted_address
            }
            query ProviderScalars(%search = {}) {
              events(
                where .event_date == %date
                  and .local_time >= %start
                  and .address in %addresses
                  and .big_id >= %minimum_big_id
                  and .big_id <= 9223372036854775807
                  and %search on selected
                order by local_time asc
                limit 1
              ) {
                event_date
                local_time
                address
                big_id
              }
            }
        "#},
    )
    .await;

    let rendered = render_sql(&bowl).await;
    let generated = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let dynamic_sites = generated
        .collect()
        .into_iter()
        .flat_map(|(_, fact)| &fact.0.dynamic_sites)
        .flat_map(|site| {
            site.fields.iter().flat_map(|field| {
                field.operators.iter().filter_map(|operator| {
                    let before = operator.before_value.as_deref()?;
                    let after = operator.after_value.as_deref().unwrap_or_default();
                    Some(format!(
                        "{}.{}: {before}<value>{after}",
                        field.key,
                        operator.name.as_str()
                    ))
                })
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(format!("{rendered}\n\n-- dynamic casts\n{dynamic_sites}"));
}

#[tokio::test]
async fn domains_and_database_arrays_use_shape_aware_json_wires() {
    let bowl = sql_bowl(structured_type_catalog()).await;
    insert_source(
        &bowl,
        "structured-types.dsql",
        indoc::indoc! {r#"
            query StructuredTypes(%label = "primary", %address = "127.0.0.1") {
              typed_values(
                where .label == %label and .address == %address
                limit 1
              ) {
                label
                address
                labels
                big_values
                addresses
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
                limit %page_limit
                offset %page_offset
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
                  or .production_year %null_operator[==, !=] null
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
              by_key: title(where .id == %id) {
                id
                latest_info: movie_info(order by id desc limit 1) { id info }
              }
              runtime: title(limit %count) { id }
              ...title(where .id == %flat_id) { flat_id: id flat_title: title }
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
                ...users(where .name like %owner) {
                  owner_name: name
                  recent: posts(limit 1) { title }
                }
              }
            }
            query FlattenPostStats {
              accounts: public::users(limit 1) {
                id
                ...posts(where .title like %title) | aggregate {
                  post_count: count
                  latest_post: max .created_at
                }
              }
            }
            query FlattenRoot {
              ...public::users(where .name == %root_name) | aggregate {
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

    let plans = bowl.scoop::<Query<(Entity, &QueryPlanFact)>>().await;
    let plans = plans.collect();
    assert_eq!(plans.len(), 1, "one query definition has one plan");
    assert_eq!(
        plans[0].1.0.roots.len(),
        5,
        "the definition plan retains every ordered root"
    );

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
        .map(|(_, fact)| fact.0.operation_name.clone())
        .collect();
    assert_eq!(
        names,
        vec!["Q".to_string()],
        "stale SQL must retire with the edit"
    );
}

#[tokio::test]
async fn definition_products_keep_identity_across_fragment_content_revisits() {
    let bowl = sql_bowl(imdb_catalog()).await;
    let initial = indoc::indoc! {r#"
        fragment VariableBits on title {
          id
          title
        }

        fragment RatingBits on title {
          ratings: movie_info_idx(where .info_type_id == 101 order by id asc limit 1) {
            info
          }
        }
    "#};
    let changed = initial.replace("  id\n", "  id\n  probe_year: production_year\n");
    let file = insert_source(&bowl, "variable-fragment.dsql", initial).await;
    insert_embedding_source(
        &bowl,
        "VariablePanel.ts",
        "export const variableQuery = dsql`\nquery VariableDependent { title(limit 1) { ...VariableBits } }\n`;\n",
        "typescript",
    )
    .await;
    let initial_ids = definition_product_ids(&bowl, "VariableDependent", "initial").await;

    set_source_text(&bowl, file, &changed).await;
    let changed_ids = definition_product_ids(&bowl, "VariableDependent", "changed").await;
    set_source_text(&bowl, file, initial).await;
    let restored_ids = definition_product_ids(&bowl, "VariableDependent", "restored").await;
    set_source_text(&bowl, file, &changed).await;
    let revisited_ids = definition_product_ids(&bowl, "VariableDependent", "revisited").await;

    assert_eq!(
        changed_ids, initial_ids,
        "the first edit keeps product identity"
    );
    assert_eq!(
        restored_ids, initial_ids,
        "restoration keeps product identity"
    );
    assert_eq!(
        revisited_ids, initial_ids,
        "revisiting edited content keeps product identity"
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
