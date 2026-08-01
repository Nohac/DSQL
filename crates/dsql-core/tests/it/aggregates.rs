//! Aggregate transform facts and checks across root, nested, and fragment
//! collection sources.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::entities::aggregate::{AggregateMode, ResolvedAggregate};
use dsql_core::facts::{
    Diagnostic, DiagnosticCode, DiagnosticSource, PlanDemand, Span, arm_editor_demands,
};
use dsql_core::language_bowl;
use dsql_core::resolution::ResolvedClause;
use dsql_core::source::insert_source;

use crate::render_diagnostic_facts;

async fn checked_bowl(catalog: Catalog) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    arm_editor_demands(&bowl).await;
    bowl
}

async fn render_resolved_aggregates(bowl: &Bowl) -> String {
    let aggregates = bowl.scoop::<Query<(Entity, &ResolvedAggregate)>>().await;
    let mut lines: Vec<String> = aggregates
        .collect()
        .into_iter()
        .map(|(_, aggregate)| {
            let mode = match aggregate.mode {
                AggregateMode::Ungrouped => "ungrouped",
                AggregateMode::Grouped => "grouped",
            };
            let fields = aggregate
                .fields
                .iter()
                .map(|field| {
                    let function = field
                        .function
                        .map_or("<invalid>", |function| function.label());
                    let output = field.output_name.as_deref().unwrap_or("<invalid>");
                    let data_type = field
                        .data_type
                        .map_or("<invalid>", |data_type| data_type.as_str());
                    format!(
                        "{output}: {function} -> {data_type} nullable={}",
                        field.nullable
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let group_keys = aggregate
                .group_keys
                .iter()
                .map(|key| {
                    format!(
                        "{}: {} nullable={}",
                        key.output_name,
                        key.data_type.as_str(),
                        key.nullable
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "table={:?} {mode} valid={} keys=[{group_keys}] fields=[{fields}]",
                aggregate.table,
                aggregate.is_valid(),
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

async fn render_predicate_aggregates(bowl: &Bowl) -> String {
    let clauses = bowl.scoop::<Query<(Entity, &ResolvedClause)>>().await;
    let mut lines: Vec<String> = clauses
        .collect()
        .into_iter()
        .flat_map(|(_, clause)| clause.aggregates.iter())
        .map(|aggregate| {
            let relation = aggregate
                .relation
                .as_ref()
                .map_or("<invalid>", |relation| relation.display.as_str());
            let function = aggregate
                .function
                .map_or("<invalid>", |function| function.label());
            let data_type = aggregate
                .data_type
                .map_or("<invalid>", |data_type| data_type.as_str());
            format!(
                "{}..{} {relation} | {function} operand={:?} -> {data_type} nullable={} valid={}",
                aggregate.span.start,
                aggregate.span.end,
                aggregate.operand,
                aggregate.nullable,
                aggregate.is_valid(),
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[tokio::test]
async fn root_nested_and_fragment_aggregates_resolve_from_one_semantic_ir() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "aggregates.dsql",
        indoc::indoc! {r#"
            fragment UserSummary on public::users {
              post_stats: posts | aggregate {
                count
              }
              post_titles: posts | aggregate by title_group: .title {
                count
                latest: max .created_at
              }
            }
            query Summaries {
              stats: public::users(where .name like "A%") | aggregate {
                count
                populated_email: count .email
                any: exists
                first_name: min .name
                latest_signup: max .created_at
              }
              by_name: public::users | aggregate by label: .name, .email {
                count
                latest_signup: max .created_at
              }
              public::users(limit 2) {
                id
                post_stats: posts(where .title like "%x%") | aggregate {
                  count
                  latest: max .created_at
                }
              }
            }
        "#},
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_resolved_aggregates(&bowl).await);
}

#[tokio::test]
async fn invalid_aggregate_contracts_report_typed_diagnostics() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "invalid-aggregates.dsql",
        indoc::indoc! {r#"
            query InvalidAggregates {
              unknown: public::users | summarize { count }
              grouped_exists: public::users | aggregate by .name { exists }
              grouped_path: public::users | aggregate by ..name { count }
              grouped_collision: public::users | aggregate by count: .name { count }
              ...public::users | aggregate by .name { count }
              empty: public::users | aggregate {}
              fields: public::users | aggregate {
                mystery
                count .id
                any: exists .id
                min
                traversed: max ..name
                uuid_min: min .id
                text_sum: sum .name
                count
                count
                aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa: count
              }
              sliced: public::users(order by name limit 1 offset 1) | aggregate { count }
              public::users(limit 1) {
                scalar: name | aggregate { count }
              }
              public::posts(limit 1) {
                singular: users | aggregate { count }
              }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn unresolved_aggregate_roots_report_one_primary_diagnostic() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;
    bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
        .await;
    insert_source(
        &bowl,
        "unresolved-aggregate.dsql",
        "query Broken { mvie_info_idx | aggregate { count } }\n",
    )
    .await;

    let diagnostics = bowl
        .scoop::<Query<(
            Entity,
            &DiagnosticSource,
            &DiagnosticCode,
            &Span,
            &Diagnostic,
        )>>()
        .await;
    let mut rendered = diagnostics
        .collect()
        .into_iter()
        .map(|(_, source, code, span, diagnostic)| {
            format!(
                "{source:?} {code:?}[{}..{}]: {}",
                span.start, span.end, diagnostic.0
            )
        })
        .collect::<Vec<_>>();
    rendered.sort();

    insta::assert_snapshot!(rendered.join("\n"));
}

#[tokio::test]
async fn scalar_predicate_aggregates_share_selection_function_semantics() {
    let bowl = checked_bowl(crate::imdb_catalog()).await;
    insert_source(
        &bowl,
        "aggregate-predicates.dsql",
        indoc::indoc! {r#"
            query AggregatePredicates {
              title(
                where .movie_info_idx | exists
                  and .movie_info_idx | count >= %minimum
                  and (.movie_info_idx | count .info) >= 1
                  and (.movie_info_idx | min .info) like "4.%"
                  and (.movie_info_idx | max .info) != null
                  and (.movie_info_idx | sum .info_type_id) > 0
                  and (.movie_info_idx | avg .info_type_id) > 0
                  and (.aka_title->movie_id | count) >= 0
                limit 1
              ) { id }
            }
        "#},
    )
    .await;

    assert_eq!(render_diagnostic_facts(&bowl).await, "");
    insta::assert_snapshot!(render_predicate_aggregates(&bowl).await);
}

#[tokio::test]
async fn invalid_scalar_predicate_aggregates_report_typed_diagnostics() {
    let bowl = checked_bowl(crate::imdb_catalog()).await;
    insert_source(
        &bowl,
        "invalid-aggregate-predicates.dsql",
        indoc::indoc! {r#"
            query InvalidPredicates @.include_if(if: .movie_info_idx | exists) {
              title(
                where ..movie_info_idx | count > 0
                  and .movie_info_idx.title | count > 0
                  and .kind_type | count > 0
                  and .id | count > 0
                  and .movie_info_idx | mystery > 0
                  and .movie_info_idx | min > 0
                  and .movie_info_idx | exists .info
                  and .movie_info_idx | sum .info > 0
                  and .movie_info_idx | exists > 1
                  and .movie_info_idx | count
                  and .movie_info_idx | count %operator[==, >] 1
                  and .movie_info_idx.info == (.aka_title->movie_id | count)
                  and (.aka_title->movie_id | count) == .movie_info_idx.info
                limit .movie_info_idx | count
              ) { id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}
