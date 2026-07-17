//! Completion: the grammar layer supplies keywords from the parser's
//! expected tokens, the site layer classifies the cursor, and entities
//! contribute tables, columns, relations, and fragments for the resolved
//! context table.

use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::language_bowl;
use dsql_core::service::{CompletionList, CompletionRequest, Position};
use dsql_core::source::{FilePath, insert_source};

async fn completions(source_with_cursor: &str) -> String {
    completions_with_marker(source_with_cursor, '|').await
}

async fn completions_with_marker(source_with_cursor: &str, marker: char) -> String {
    let offset = source_with_cursor
        .find(marker)
        .expect("test source marks the cursor with |");
    let source = source_with_cursor.replacen(marker, "", 1);

    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    insert_source(&bowl, "test.dsql", &source).await;

    let list = bowl
        .insert((
            CompletionRequest,
            FilePath("test.dsql".to_string()),
            Position { offset },
        ))
        .await
        .bind()
        .take::<CompletionList>()
        .await
        .expect("completion requests with a known file are answered");

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
            format!("{:?} {}{detail}{insert}", item.kind, item.label)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn document_root_offers_definition_keywords() {
    insta::assert_snapshot!(completions("|").await);
}

#[tokio::test]
async fn root_selection_offers_tables() {
    insta::assert_snapshot!(completions("query Q {\n  |\n}\n").await);
}

#[tokio::test]
async fn selection_body_offers_columns_relations_and_fragments() {
    insta::assert_snapshot!(
        completions("fragment UserBits on users {\n  id\n}\nquery Q {\n  users {\n    |\n  }\n}\n")
            .await
    );
}

#[tokio::test]
async fn clause_list_offers_clause_keywords() {
    insta::assert_snapshot!(completions("query Q {\n  users(|) {\n    id\n  }\n}\n").await);
}

#[tokio::test]
async fn where_offers_scopes_columns_and_literals() {
    insta::assert_snapshot!(completions("query Q {\n  users(where |) {\n    id\n  }\n}\n").await);
}

#[tokio::test]
async fn where_after_anchor_offers_columns() {
    insta::assert_snapshot!(completions("query Q {\n  users(where .|) {\n    id\n  }\n}\n").await);
}

#[tokio::test]
async fn where_after_path_offers_operators() {
    insta::assert_snapshot!(
        completions("query Q {\n  users(where .id |) {\n    id\n  }\n}\n").await
    );
}

#[tokio::test]
async fn spread_offers_matching_fragments() {
    insta::assert_snapshot!(
            completions(
                "fragment UserBits on users {\n  id\n}\nfragment PostBits on posts {\n  id\n}\nquery Q {\n  users {\n    ...|\n  }\n}\n"
            )
            .await
        );
}

#[tokio::test]
async fn order_by_offers_columns_and_directions() {
    insta::assert_snapshot!(
        completions("query Q {\n  users(order by |) {\n    id\n  }\n}\n").await
    );
}

#[tokio::test]
async fn aggregate_bodies_offer_contextual_functions_and_operands() {
    let functions =
        completions_with_marker("query Q {\n  users | aggregate {\n    ¦\n  }\n}\n", '¦').await;
    let operands = completions_with_marker(
        "query Q {\n  users | aggregate {\n    value: min .¦\n  }\n}\n",
        '¦',
    )
    .await;
    let group_keys =
        completions_with_marker("query Q {\n  users | aggregate by .n¦ { count }\n}\n", '¦').await;

    insta::assert_snapshot!(format!(
        "functions:\n{functions}\n\noperands:\n{operands}\n\ngroup keys:\n{group_keys}"
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
        "query Q {\n  users(limit 1) {\n    ...posts | aggregate {\n      ¦\n    }\n  }\n}\n",
        '¦',
    )
    .await;

    insta::assert_snapshot!(format!(
        "singular body:\n{singular}\n\nclause:\n{clause}\n\naggregate body:\n{aggregate}"
    ));
}
