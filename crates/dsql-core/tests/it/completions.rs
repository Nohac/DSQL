//! Completion: the grammar layer supplies keywords from the parser's
//! expected tokens, the site layer classifies the cursor, and entities
//! contribute tables, columns, relations, and fragments for the resolved
//! context table.

use bowl::Bowl;
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::register_language;
use dsql_core::service::{CompletionList, CompletionRequest, Position};
use dsql_core::source::{FilePath, insert_source};
use futures::executor::block_on;

async fn completions(source_with_cursor: &str) -> String {
    let offset = source_with_cursor
        .find('|')
        .expect("test source marks the cursor with |");
    let source = source_with_cursor.replace('|', "");

    let bowl = Bowl::new();
    register_language(&bowl).await;
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

    list.0
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

#[test]
fn document_root_offers_definition_keywords() {
    block_on(async {
        insta::assert_snapshot!(completions("|").await);
    });
}

#[test]
fn root_selection_offers_tables() {
    block_on(async {
        insta::assert_snapshot!(completions("query Q {\n  |\n}\n").await);
    });
}

#[test]
fn selection_body_offers_columns_relations_and_fragments() {
    block_on(async {
        insta::assert_snapshot!(
            completions(
                "fragment UserBits on users {\n  id\n}\nquery Q {\n  users {\n    |\n  }\n}\n"
            )
            .await
        );
    });
}

#[test]
fn clause_list_offers_clause_keywords() {
    block_on(async {
        insta::assert_snapshot!(completions("query Q {\n  users(|) {\n    id\n  }\n}\n").await);
    });
}

#[test]
fn where_offers_scopes_columns_and_literals() {
    block_on(async {
        insta::assert_snapshot!(
            completions("query Q {\n  users(where |) {\n    id\n  }\n}\n").await
        );
    });
}

#[test]
fn where_after_anchor_offers_columns() {
    block_on(async {
        insta::assert_snapshot!(
            completions("query Q {\n  users(where .|) {\n    id\n  }\n}\n").await
        );
    });
}

#[test]
fn where_after_path_offers_operators() {
    block_on(async {
        insta::assert_snapshot!(
            completions("query Q {\n  users(where .id |) {\n    id\n  }\n}\n").await
        );
    });
}

#[test]
fn spread_offers_matching_fragments() {
    block_on(async {
        insta::assert_snapshot!(
            completions(
                "fragment UserBits on users {\n  id\n}\nfragment PostBits on posts {\n  id\n}\nquery Q {\n  users {\n    ...|\n  }\n}\n"
            )
            .await
        );
    });
}

#[test]
fn order_by_offers_columns_and_directions() {
    block_on(async {
        insta::assert_snapshot!(
            completions("query Q {\n  users(order by |) {\n    id\n  }\n}\n").await
        );
    });
}
