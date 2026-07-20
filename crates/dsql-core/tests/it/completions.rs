//! Completion: the grammar layer supplies keywords from the parser's
//! expected tokens, the site layer classifies the cursor, and entities
//! contribute tables, columns, relations, and fragments for the resolved
//! context table.

use std::sync::Arc;

use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::language_bowl;
use dsql_core::service::{CompletionList, CompletionRequest, Position};
use dsql_core::source::{FilePath, insert_source};

async fn completions(source_with_cursor: &str) -> String {
    completions_with_marker(source_with_cursor, '|').await
}

async fn completions_with_marker(source_with_cursor: &str, marker: char) -> String {
    let list = completion_list(source_with_cursor, marker).await;
    render_completion_items(&list)
}

async fn completion_list(source_with_cursor: &str, marker: char) -> Arc<CompletionList> {
    completion_list_with_catalog(source_with_cursor, marker, Catalog::hardcoded()).await
}

async fn completion_list_with_catalog(
    source_with_cursor: &str,
    marker: char,
    catalog: Catalog,
) -> Arc<CompletionList> {
    let offset = source_with_cursor
        .find(marker)
        .expect("test source marks the cursor with |");
    let source = source_with_cursor.replacen(marker, "", 1);

    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    insert_source(&bowl, "test.dsql", &source).await;

    bowl.insert((
        CompletionRequest,
        FilePath("test.dsql".to_string()),
        Position { offset },
    ))
    .await
    .bind()
    .take::<CompletionList>()
    .await
    .expect("completion requests with a known file are answered")
}

#[tokio::test]
async fn contextual_identifier_columns_complete_and_keep_predicate_context() {
    let fields =
        completion_list_with_catalog("query Q { metrics { | } }", '|', crate::numeric_catalog())
            .await;
    let operators = completion_list_with_catalog(
        "query Q { metrics(where .exists |) { exists } }",
        '|',
        crate::numeric_catalog(),
    )
    .await;

    insta::assert_snapshot!(format!(
        "fields:\n{}\n\nafter .exists:\n{}",
        render_completion_items(&fields),
        render_completion_items(&operators),
    ));
}

fn render_completion_items(list: &CompletionList) -> String {
    list.items
        .iter()
        .map(|item| {
            let detail = item
                .detail
                .as_deref()
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default();
            let insert = item
                .insert_text
                .as_deref()
                .map(|text| format!(" insert={text}"))
                .unwrap_or_default();
            let documentation = item
                .documentation
                .as_deref()
                .map(|documentation| format!("\n  {documentation}"))
                .unwrap_or_default();
            format!(
                "{:?} {}{detail}{insert}{documentation}",
                item.kind, item.label
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn catalog_descriptions_reach_table_and_column_completions() {
    let mut catalog = Catalog::hardcoded();
    catalog.tables[0].description = Some("Application accounts.".to_string());
    catalog.columns[1].description = Some("The account's public display name.".to_string());

    let tables = completion_list_with_catalog("query Q { | }", '|', catalog.clone()).await;
    let columns =
        completion_list_with_catalog("query Q { public::users { | } }", '|', catalog).await;

    insta::assert_snapshot!(format!(
        "tables:\n{}\n\ncolumns:\n{}",
        render_completion_items(&tables),
        render_completion_items(&columns),
    ));
}

#[tokio::test]
async fn document_root_offers_definition_keywords() {
    insta::assert_snapshot!(completions("|").await);
}

#[tokio::test]
async fn filter_declarations_complete_structural_targets_and_body_rules() {
    let catalog = crate::policy_completion_catalog();
    let unrooted_target_fields = completion_list_with_catalog(
        "filter MovieFilter on { | } { where true }",
        '|',
        catalog.clone(),
    )
    .await;
    let target_fields = completion_list_with_catalog(
        "filter MovieFilter on { .| } { where true }",
        '|',
        catalog.clone(),
    )
    .await;
    let target_types = completion_list_with_catalog(
        "filter MovieFilter on { .nr_order: | } { where true }",
        '|',
        catalog.clone(),
    )
    .await;
    let before_colon = completion_list_with_catalog(
        "filter MovieFilter on { .nr_order | } { where true }",
        '|',
        catalog.clone(),
    )
    .await;
    let body_rules = completion_list_with_catalog(
        "filter MovieFilter on { .nr_order: int } { | }",
        '|',
        catalog.clone(),
    )
    .await;
    let body_fields = completion_list_with_catalog(
        concat!(
            "filter MovieFilter on {\n",
            "  .nr_order: int\n",
            "  .shared: text\n",
            "} {\n",
            "  where .|\n",
            "}\n",
        ),
        '|',
        catalog.clone(),
    )
    .await;
    let concrete_body_fields =
        completion_list_with_catalog("filter MovieFilter on first { where .| }", '|', catalog)
            .await;

    insta::assert_snapshot!(format!(
        "unrooted target fields:\n{}\n\ntarget fields:\n{}\n\ntarget types:\n{}\n\nbefore colon:\n{}\n\nbody rules:\n{}\n\nshape body fields:\n{}\n\nconcrete body fields:\n{}",
        render_completion_items(&unrooted_target_fields),
        render_completion_items(&target_fields),
        render_completion_items(&target_types),
        render_completion_items(&before_colon),
        render_completion_items(&body_rules),
        render_completion_items(&body_fields),
        render_completion_items(&concrete_body_fields),
    ));
}

#[tokio::test]
async fn root_selection_offers_tables() {
    insta::assert_snapshot!(completions("query Q {\n  |\n}\n").await);
}

#[tokio::test]
async fn selection_body_offers_columns_relations_and_fragments() {
    insta::assert_snapshot!(
        completions("fragment UserBits on public::users {\n  id\n}\nquery Q {\n  public::users {\n    |\n  }\n}\n")
            .await
    );
}

#[tokio::test]
async fn clause_list_offers_clause_keywords() {
    insta::assert_snapshot!(completions("query Q {\n  public::users(|) {\n    id\n  }\n}\n").await);
}

#[tokio::test]
async fn where_offers_scopes_columns_and_literals() {
    insta::assert_snapshot!(
        completions("query Q {\n  public::users(where |) {\n    id\n  }\n}\n").await
    );
}

#[tokio::test]
async fn where_after_anchor_offers_columns() {
    insta::assert_snapshot!(
        completions("query Q {\n  public::users(where .|) {\n    id\n  }\n}\n").await
    );
}

#[tokio::test]
async fn where_after_path_offers_operators() {
    insta::assert_snapshot!(
        completions("query Q {\n  public::users(where .id |) {\n    id\n  }\n}\n").await
    );
}

#[tokio::test]
async fn spread_offers_matching_fragments() {
    insta::assert_snapshot!(
            completions(
                "fragment UserBits on public::users {\n  id\n}\nfragment PostBits on posts {\n  id\n}\nquery Q {\n  public::users {\n    ...|\n  }\n}\n"
            )
            .await
        );
}

#[tokio::test]
async fn order_by_offers_columns_and_directions() {
    insta::assert_snapshot!(
        completions("query Q {\n  public::users(order by |) {\n    id\n  }\n}\n").await
    );
}

#[tokio::test]
async fn aggregate_bodies_offer_contextual_functions_and_operands() {
    let functions = completions_with_marker(
        "query Q {\n  public::users | aggregate {\n    ¦\n  }\n}\n",
        '¦',
    )
    .await;
    let operands = completions_with_marker(
        "query Q {\n  public::users | aggregate {\n    value: min .¦\n  }\n}\n",
        '¦',
    )
    .await;
    let pipe = completion_list("query Q {\n  public::users | ¦\n}\n", '¦').await;
    let partial_pipe =
        completion_list("query Q {\n  public::users | aggr¦ { count }\n}\n", '¦').await;
    let group_keys = completions_with_marker(
        "query Q {\n  public::users | aggregate by ¦ { count }\n}\n",
        '¦',
    )
    .await;
    let rooted_group_keys = completions_with_marker(
        "query Q {\n  public::users | aggregate by .n¦ { count }\n}\n",
        '¦',
    )
    .await;

    insta::assert_snapshot!(format!(
        "pipe replace={:?}:\n{}\n\npartial pipe replace={:?}:\n{}\n\nfunctions:\n{functions}\n\noperands:\n{operands}\n\ngroup keys:\n{group_keys}\n\nrooted group keys:\n{rooted_group_keys}",
        pipe.replace,
        render_completion_items(&pipe),
        partial_pipe.replace,
        render_completion_items(&partial_pipe),
    ));
}

#[tokio::test]
async fn aggregate_predicates_offer_contextual_functions_and_related_operands() {
    let functions = completion_list(
        "query Q {\n  public::users(where .posts | ¦ limit 1) { id }\n}\n",
        '¦',
    )
    .await;
    let partial = completion_list(
        "query Q {\n  public::users(where .posts | co¦ >= 1 limit 1) { id }\n}\n",
        '¦',
    )
    .await;
    let operands = completions_with_marker(
        "query Q {\n  public::users(where (.posts | min .¦) == \"a\" limit 1) { id }\n}\n",
        '¦',
    )
    .await;

    insta::assert_snapshot!(format!(
        "functions replace={:?}:\n{}\n\npartial replace={:?}:\n{}\n\noperands:\n{operands}",
        functions.replace,
        render_completion_items(&functions),
        partial.replace,
        render_completion_items(&partial),
    ));
}

#[tokio::test]
async fn flattened_selections_complete_against_their_relation_context() {
    let singular = completions_with_marker(
        "query Q {\n  posts(limit 1) {\n    ...users {\n      ¦\n    }\n  }\n}\n",
        '¦',
    )
    .await;
    let clause = completions_with_marker(
        "query Q {\n  posts(limit 1) {\n    ...users(where .¦) { name }\n  }\n}\n",
        '¦',
    )
    .await;
    let aggregate = completions_with_marker(
        "query Q {\n  public::users(limit 1) {\n    ...posts | aggregate {\n      ¦\n    }\n  }\n}\n",
        '¦',
    )
    .await;

    insta::assert_snapshot!(format!(
        "singular body:\n{singular}\n\nclause:\n{clause}\n\naggregate body:\n{aggregate}"
    ));
}
