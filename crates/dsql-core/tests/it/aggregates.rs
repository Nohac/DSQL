//! Aggregate transform facts and checks across root, nested, and fragment
//! collection sources.

use bowl::{Bowl, Entity, Query};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::entities::aggregate::{AggregateMode, ResolvedAggregate};
use dsql_core::facts::arm_editor_demands;
use dsql_core::language_bowl;
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

#[tokio::test]
async fn root_nested_and_fragment_aggregates_resolve_from_one_semantic_ir() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "aggregates.dsql",
        concat!(
            "fragment UserSummary on public::users {\n",
            "  post_stats: posts | aggregate {\n",
            "    count\n",
            "  }\n",
            "  post_titles: posts | aggregate by title_group: .title {\n",
            "    count\n",
            "    latest: max .created_at\n",
            "  }\n",
            "}\n",
            "query Summaries {\n",
            "  stats: public::users(where .name like \"A%\") | aggregate {\n",
            "    count\n",
            "    populated_email: count .email\n",
            "    any: exists\n",
            "    first_name: min .name\n",
            "    latest_signup: max .created_at\n",
            "  }\n",
            "  by_name: public::users | aggregate by label: .name, .email {\n",
            "    count\n",
            "    latest_signup: max .created_at\n",
            "  }\n",
            "  public::users(limit 2) {\n",
            "    id\n",
            "    post_stats: posts(where .title like \"%x%\") | aggregate {\n",
            "      count\n",
            "      latest: max .created_at\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
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
        concat!(
            "query InvalidAggregates {\n",
            "  unknown: public::users | summarize { count }\n",
            "  grouped_exists: public::users | aggregate by .name { exists }\n",
            "  grouped_path: public::users | aggregate by ..name { count }\n",
            "  grouped_collision: public::users | aggregate by count: .name { count }\n",
            "  ...public::users | aggregate by .name { count }\n",
            "  empty: public::users | aggregate {}\n",
            "  fields: public::users | aggregate {\n",
            "    mystery\n",
            "    count .id\n",
            "    any: exists .id\n",
            "    min\n",
            "    traversed: max ..name\n",
            "    uuid_min: min .id\n",
            "    count\n",
            "    count\n",
            "    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa: count\n",
            "  }\n",
            "  sliced: public::users(order by name limit 1 offset 1) | aggregate { count }\n",
            "  public::users(limit 1) {\n",
            "    scalar: name | aggregate { count }\n",
            "  }\n",
            "  public::posts(limit 1) {\n",
            "    singular: users | aggregate { count }\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}
