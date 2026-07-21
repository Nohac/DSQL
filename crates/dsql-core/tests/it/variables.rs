//! Variable inference: structured (`$`)
//! versus top-level (`$$`) paths, operator allowlists, sort directions, and
//! fragment envelopes. Demand-gated on `VariablesDemand`.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::variable::{DefinitionVariables, VariableBinding};
use dsql_core::facts::{DefKey, Span, VariablesDemand, arm_editor_demands};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;

use crate::{imdb_catalog, render_diagnostic_facts};

async fn variables_bowl() -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    bowl
}

/// Renders bindings one per line for snapshots.
pub(crate) async fn render_bindings(bowl: &Bowl) -> String {
    let rows = bowl
        .scoop::<Query<(Entity, &Span, &VariableBinding)>>()
        .await;
    let mut bindings: Vec<(&Span, &VariableBinding)> = rows
        .collect()
        .into_iter()
        .map(|(_, span, binding)| (span, binding))
        .collect();
    bindings.sort_by_key(|(span, _)| (span.start, span.end));
    bindings
        .into_iter()
        .map(|(_, binding)| {
            let operators = binding
                .operators
                .iter()
                .map(|operator| format!("{operator:?}"))
                .collect::<Vec<_>>()
                .join("|");
            let collection = if binding.collection {
                " collection"
            } else {
                ""
            };
            format!(
                "{} {:?} {:?} {:?} {:?}{collection} operators=[{}] enum=[{}]",
                binding.path,
                binding.source,
                binding.role,
                binding.data_type,
                binding.name,
                operators,
                binding.enum_values.join("|")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn render_definition_bindings(bowl: &Bowl) -> String {
    let definitions = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let contract_rows = bowl
        .scoop::<Query<(Entity, &DefinitionVariables, &DefKey)>>()
        .await;
    let contracts = contract_rows.collect();
    let mut rendered = definitions
        .collect()
        .into_iter()
        .filter_map(|(entity, definition)| {
            let (_, contract, _) = contracts.iter().find(|(_, _, key)| key.0 == entity)?;
            let bindings = contract
                .0
                .iter()
                .map(|binding| {
                    format!(
                        "  {} {:?} {:?} {:?} required={} nullable={} default={:?}",
                        binding.path,
                        binding.source,
                        binding.role,
                        binding.data_type,
                        binding.required,
                        binding.nullable,
                        binding.default,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Some((definition.name.clone(), bindings))
        })
        .collect::<Vec<_>>();
    rendered.sort_by(|left, right| left.0.cmp(&right.0));
    rendered
        .into_iter()
        .map(|(name, bindings)| format!("{name}\n{bindings}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn case(name: &str, source: &str) -> String {
    let bowl = variables_bowl().await;
    insert_source(&bowl, format!("{name}.dsql"), source).await;
    format!("{name}\n{}", render_bindings(&bowl).await)
}

/// The four reference inference cases.
#[tokio::test]
async fn variable_bindings_match_reference_shapes() {
    let scalar = case(
            "scalar",
            "\nquery VariableSearch {\n  public::users(where .id > $min_id and .posts.title like $$title limit $ offset $$) {\n    id\n    posts(limit $post_limit) {\n      title\n    }\n  }\n}\n",
        )
        .await;
    let operator = case(
            "operator",
            "\nquery PostSearch {\n  posts(where .created_at $[>, >=] $min_created_at and .title $$title_op[==, !=, like] $title limit $) {\n    id\n  }\n}\n",
        )
        .await;
    let order_by_direction = case(
            "order_by_direction",
            "\nquery PostOrdering {\n  posts(order by created_at $created_dir, title $$) {\n    id\n  }\n}\n",
        )
        .await;
    let fragment = case(
            "fragment",
            "\nfragment UserPosts on public::users {\n  posts(where .title like $$search limit $post_limit) {\n    title\n  }\n}\n",
        )
        .await;

    insta::assert_snapshot!(format!(
        "{scalar}\n\n---\n\n{operator}\n\n---\n\n{order_by_direction}\n\n---\n\n{fragment}"
    ));
}

/// Fragment spreads nested inside fragment bodies expand with an enveloped
/// path scope (`input.<selection>.body.<Fragment>.params...`).
#[tokio::test]
async fn fragment_spread_envelopes_nested_bindings() {
    let bowl = variables_bowl().await;
    insert_source(
        &bowl,
        "envelope.dsql",
        concat!(
            "fragment UserFilter on public::users {\n",
            "  ...UserPosts\n",
            "}\n",
            "fragment UserPosts on public::users {\n",
            "  recent: posts(limit $count) {\n    id\n  }\n",
            "}\n",
        ),
    )
    .await;
    insta::assert_snapshot!(render_definition_bindings(&bowl).await);
}

#[tokio::test]
async fn fragment_inputs_are_contained_lifted_namespaced_and_defaulted() {
    let bowl = variables_bowl().await;
    insert_source(
        &bowl,
        "fragment-inputs.dsql",
        concat!(
            "fragment UserPanel($created_after? = null $$limit = 10) on public::users {\n",
            "  posts(where .created_at > $created_after limit $$) { id }\n",
            "}\n",
            "query Contained { public::users { ...UserPanel } }\n",
            "query Lifted($$page_size = 20) {\n",
            "  public::users { ...UserPanel($, $$limit <- $$page_size) }\n",
            "}\n",
            "query Namespaced {\n",
            "  public::users {\n",
            "    ...UserPanel($ <- $$panel_input, $$ <- $$panel_params)\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_definition_bindings(&bowl).await);
}

#[tokio::test]
async fn flattened_aggregate_inputs_keep_the_source_and_aggregate_path_segments() {
    let snapshot = case(
        "flattened",
        concat!(
            "query FlattenedInputs {\n",
            "  ...public::users(where .name == $root_name) | aggregate { user_count: count }\n",
            "  accounts: public::users(limit 1) {\n",
            "    ...posts(where .title == $post_title) | aggregate { post_count: count }\n",
            "  }\n",
            "  feed: posts(limit 1) {\n",
            "    ...users(where .name == $owner_name) { owner_name: name }\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(snapshot);
}

#[tokio::test]
async fn aggregate_predicate_inputs_use_resolved_result_types_and_paths() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    insert_source(
        &bowl,
        "aggregate-predicate-inputs.dsql",
        concat!(
            "query AggregatePredicateInputs {\n",
            "  title(\n",
            "    where (.movie_info_idx | count) >= $minimum\n",
            "      and (.movie_info_idx | min .info) >= $$earliest\n",
            "      and (.movie_info_idx | sum .info_type_id) >= $total\n",
            "    limit 1\n",
            "  ) { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_bindings(&bowl).await);
}

#[tokio::test]
async fn predicate_extensions_infer_boolean_scalar_and_collection_bindings() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_source(
        &bowl,
        "predicate-bindings.dsql",
        concat!(
            "query PredicateBindings {\n",
            "  title(where not $$disabled and .id in $$ids\n",
            "    and exists .movie_info_idx(where .info_type_id in $:allowed_types)) { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(format!(
        "diagnostics:\n{}\n\nbindings:\n{}",
        render_diagnostic_facts(&bowl).await,
        render_bindings(&bowl).await,
    ));
}

#[tokio::test]
async fn invalid_aggregate_predicates_do_not_infer_bindings() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_source(
        &bowl,
        "invalid-aggregate-predicate-input.dsql",
        concat!(
            "query InvalidAggregateInput {\n",
            "  title(where (.kind_type | count) >= $minimum limit 1) { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(format!(
        "diagnostics:\n{}\n\nbindings:\n{}",
        render_diagnostic_facts(&bowl).await,
        render_bindings(&bowl).await,
    ));
}

#[tokio::test]
async fn cross_file_fragment_clauses_preserve_variable_inference() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    insert_source(
        &bowl,
        "rating-filter.dsql",
        concat!(
            "fragment RatingFilter on title {\n",
            "  ratings: movie_info_idx(\n",
            "    where .info_type_id == $type\n",
            "    order by id $direction\n",
            "    limit $count\n",
            "  ) {\n",
            "    info\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;
    insert_source(
        &bowl,
        "ranked-fields.dsql",
        "fragment RankedFields on title {\n  ...RatingFilter\n}\n",
    )
    .await;

    insta::assert_snapshot!(render_definition_bindings(&bowl).await);
}
