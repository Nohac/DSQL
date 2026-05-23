mod analysis;
mod completion;
mod cursor;
mod db;
mod definition;
mod document;
mod host;
mod hover;
mod provider;
mod semantic_tokens;

pub use analysis::{analyze_source, collect_diagnostics};
pub use completion::{CompletionItem, CompletionKind};
pub use db::{AnalysisResult, ParsedFile};
pub use definition::{CatalogDefinition, DefinitionResult, SourceDefinition, SourceDefinitionKind};
pub use document::{
    DocumentDiagnostics, DocumentFormat, DocumentSnapshot, FileId, RevisionId, TextEdit,
    TextEditRange, TextPosition,
};
pub use host::AnalysisHost;
pub use hover::HoverInfo;
pub use provider::{CatalogProvider, HardcodedCatalogProvider};
pub use semantic_tokens::{DocumentSemanticTokens, SemanticTokenInfo, SemanticTokenKind};

fn range_contains(range: dsql_core::TextRange, byte: usize) -> bool {
    (range.start as usize) <= byte && byte <= (range.end as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CompilerDb;
    use dsql_core::{Catalog, ParseResult, SourceFile, parse_source};
    use ropey::Rope;
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match Pin::new(&mut future).poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn noop_waker() -> Waker {
        fn clone(_: *const ()) -> RawWaker {
            raw_waker()
        }
        fn wake(_: *const ()) {}
        fn wake_by_ref(_: *const ()) {}
        fn drop(_: *const ()) {}
        fn raw_waker() -> RawWaker {
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(clone, wake, wake_by_ref, drop),
            )
        }
        // SAFETY: the raw waker does not dereference its data pointer and all
        // vtable functions are no-ops suitable for polling immediately-ready futures.
        unsafe { Waker::from_raw(raw_waker()) }
    }

    fn parsed_source(source: &str) -> SourceFile {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        parsed.source_file
    }

    fn parsed(source: &str) -> ParseResult {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        parsed
    }

    #[test]
    fn completions_include_exact_relation_names() {
        let source = "query Q { posts {  } }";
        let parsed = parsed(source);
        let catalog = Catalog::hardcoded();
        let byte = source.find("  }").unwrap() + 1;

        let completions = completion::completions_at_empty_scope(&parsed, &catalog, byte);

        assert!(completions.iter().any(|completion| {
            completion.label == "users" && completion.kind == completion::CompletionKind::Relation
        }));
        assert!(
            !completions
                .iter()
                .any(|completion| completion.label == "user")
        );
    }

    #[test]
    fn root_completions_qualify_non_public_tables() {
        let source = "query Q {  }";
        let parsed = parsed(source);
        let catalog = Catalog::hardcoded();
        let byte = source.find("  }").unwrap() + 1;

        let completions = completion::completions_at_empty_scope(&parsed, &catalog, byte);

        assert!(completions.iter().any(|completion| {
            completion.label == "users" && completion.kind == completion::CompletionKind::Table
        }));
        assert!(completions.iter().any(|completion| {
            completion.label == "other_schema.users"
                && completion.kind == completion::CompletionKind::Table
        }));
    }

    #[test]
    fn completions_and_hover_work_inside_fragments() {
        let source = "fragment UserFields on public.users {\n  posts {\n    title\n  }\n}";
        let parsed = parsed(source);
        let source_file = parsed.source_file.clone();
        let catalog = Catalog::hardcoded();
        let completion_byte = source.find("  posts").unwrap() + 1;
        let hover_byte = source.find("title").unwrap();

        let completions =
            completion::completions_at_empty_scope(&parsed, &catalog, completion_byte);
        let hover = hover::hover_at(&source_file, &catalog, hover_byte).unwrap();

        assert!(completions.iter().any(|completion| {
            completion.label == "posts" && completion.kind == completion::CompletionKind::Relation
        }));
        assert_eq!(hover.label, "title");
        assert_eq!(hover.detail, "column: text");
        assert!(hover.markdown.contains("Primary key: no"));
        assert!(hover.markdown.contains("Indexed: no"));
    }

    #[test]
    fn hover_and_completions_support_qualified_names() {
        let source = "query Q { public.users { public.posts { title } } }";
        let parsed = parsed(source);
        let source_file = parsed.source_file.clone();
        let catalog = Catalog::hardcoded();
        let completion_byte = source.find("public.posts").unwrap() - 1;
        let hover_byte = source.find("public.posts").unwrap();

        let completions =
            completion::completions_at_empty_scope(&parsed, &catalog, completion_byte);
        let hover = hover::hover_at(&source_file, &catalog, hover_byte).unwrap();

        assert!(completions.iter().any(|completion| {
            completion.label == "posts" && completion.kind == completion::CompletionKind::Relation
        }));
        assert_eq!(hover.label, "public.posts");
        assert_eq!(hover.detail, "relation: public.posts");
        assert!(hover.markdown.contains("Foreign key"));
        assert!(hover.markdown.contains("posts.user_id"));
        assert!(hover.markdown.contains("users.id"));
    }

    #[test]
    fn semantic_tokens_classify_schema_tables_relations_and_columns() {
        let source = "query Q { public.users { id posts { title } } }";
        let parse = parse_source(source.into());
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        let catalog = Catalog::hardcoded();

        let tokens = semantic_tokens::semantic_tokens_at(&parse, &catalog);

        assert!(tokens.iter().any(|token| {
            token.kind == semantic_tokens::SemanticTokenKind::Schema
                && parse.source.text(token.range).as_ref() == "public"
        }));
        assert!(tokens.iter().any(|token| {
            token.kind == semantic_tokens::SemanticTokenKind::Table
                && parse.source.text(token.range).as_ref() == "users"
        }));
        assert!(tokens.iter().any(|token| {
            token.kind == semantic_tokens::SemanticTokenKind::Relation
                && parse.source.text(token.range).as_ref() == "posts"
        }));
        assert!(tokens.iter().any(|token| {
            token.kind == semantic_tokens::SemanticTokenKind::Column
                && parse.source.text(token.range).as_ref() == "title"
        }));
    }

    #[test]
    fn completion_scope_collects_fragments_from_indexed_files() {
        block_on(async {
            let db = CompilerDb::default();
            db.set_source_rope(
                FileId(0),
                RevisionId(1),
                Rope::from_str("query Q { users { id } }"),
            )
            .unwrap();
            db.set_source_rope(
                FileId(1),
                RevisionId(1),
                Rope::from_str("fragment UserFields on users { id }"),
            )
            .unwrap();

            let scope = db.completion_scope(FileId(0)).await.unwrap();

            assert_eq!(fragment_scope_names(&scope), vec!["UserFields"]);
        });
    }

    #[test]
    fn completion_scope_includes_synthetic_embedded_region_files() {
        block_on(async {
            let db = CompilerDb::default();
            let query_region = FileId(10);
            let fragment_region = FileId(11);
            db.set_source_rope(
                query_region,
                RevisionId(1),
                Rope::from_str("query Q { users { id } }"),
            )
            .unwrap();
            db.set_source_rope(
                fragment_region,
                RevisionId(1),
                Rope::from_str("fragment EmbeddedFields on users { name }"),
            )
            .unwrap();

            let scope = db.completion_scope(query_region).await.unwrap();

            assert_eq!(fragment_scope_names(&scope), vec!["EmbeddedFields"]);
        });
    }

    #[test]
    fn completion_scope_drops_fragments_when_source_is_removed() {
        block_on(async {
            let db = CompilerDb::default();
            db.set_source_rope(
                FileId(0),
                RevisionId(1),
                Rope::from_str("query Q { users { id } }"),
            )
            .unwrap();
            db.set_source_rope(
                FileId(1),
                RevisionId(1),
                Rope::from_str("fragment RemovedFields on users { id }"),
            )
            .unwrap();

            db.remove_source(FileId(1));
            let scope = db.completion_scope(FileId(0)).await.unwrap();

            assert!(scope.fragments.is_empty(), "{:?}", scope.fragments);
        });
    }

    #[test]
    fn completion_uses_updated_db_scope_without_reopening_query_file() {
        block_on(async {
            let db = CompilerDb::default();
            let query_source = "query Q { users { . } }";
            let query_file = FileId(0);
            let fragment_file = FileId(1);
            db.set_source_rope(query_file, RevisionId(1), Rope::from_str(query_source))
                .unwrap();
            db.set_source_rope(
                fragment_file,
                RevisionId(1),
                Rope::from_str("fragment OldFields on users { id }"),
            )
            .unwrap();
            db.set_source_rope(
                fragment_file,
                RevisionId(2),
                Rope::from_str("fragment NewFields on users { name }"),
            )
            .unwrap();

            let parse = parse_source(query_source.into());
            let catalog = db.catalog();
            let scope = db.completion_scope(query_file).await.unwrap();
            let completions = completion::completions_at(
                &parse,
                &catalog,
                query_source.find(".").unwrap() + 1,
                &scope,
            );
            let labels = completions
                .iter()
                .map(|completion| completion.label.as_str())
                .collect::<Vec<_>>();

            assert!(labels.contains(&"NewFields"), "{labels:?}");
            assert!(!labels.contains(&"OldFields"), "{labels:?}");
        });
    }

    #[test]
    fn hover_and_semantic_tokens_work_for_clause_columns() {
        let source = "query Q { posts(where .id > 10 order by created_at desc limit 5) { title } }";
        let source_file = parsed_source(source);
        let parse = parse_source(source.into());
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        let catalog = Catalog::hardcoded();
        let where_byte = source.find("id >").unwrap();
        let order_byte = source.find("created_at").unwrap();

        let where_hover = hover::hover_at(&source_file, &catalog, where_byte).unwrap();
        let order_hover = hover::hover_at(&source_file, &catalog, order_byte).unwrap();
        let tokens = semantic_tokens::semantic_tokens_at(&parse, &catalog);

        assert_eq!(where_hover.label, "id");
        assert_eq!(where_hover.detail, "column: uuid");
        assert_eq!(order_hover.label, "created_at");
        assert!(tokens.iter().any(|token| {
            token.kind == semantic_tokens::SemanticTokenKind::Column
                && parse.source.text(token.range).as_ref() == "created_at"
        }));
    }

    #[test]
    fn query_diagnostics_follow_changed_fragment_inputs() {
        block_on(async {
            let host = AnalysisHost::new();
            let fragment_uri = "file:///fragments.dsql".to_string();
            let query_uri = "file:///query.dsql".to_string();

            host.open_document(
                fragment_uri.clone(),
                1,
                "fragment UserFields on users { id }".to_string(),
            )
            .await;
            let initial = host
                .open_document(
                    query_uri.clone(),
                    1,
                    "query Q { users { ...UserFields } }".to_string(),
                )
                .await;
            assert!(
                initial.diagnostics.is_empty(),
                "initial diagnostics: {:?}",
                initial.diagnostics
            );

            host.replace_document(
                fragment_uri,
                2,
                "fragment UserFields on posts { title }".to_string(),
            )
            .await;
            let updated = host.document_diagnostics(&query_uri).await.unwrap();

            assert!(updated.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == dsql_core::DiagnosticCode::FragmentTypeMismatch
            }));
        });
    }

    #[test]
    fn fragment_changes_report_dependent_open_document_diagnostics() {
        block_on(async {
            let host = AnalysisHost::new();
            let fragment_uri = "file:///fragments.dsql".to_string();
            let query_uri = "file:///query.dsql".to_string();

            host.open_document(
                fragment_uri.clone(),
                1,
                "fragment UserFields on users { id }".to_string(),
            )
            .await;
            host.open_document(
                query_uri.clone(),
                1,
                "query Q { users { ...UserFields } }".to_string(),
            )
            .await;

            host.replace_document(
                fragment_uri.clone(),
                2,
                "fragment UserFields on posts { title }".to_string(),
            )
            .await;
            let related = host.open_document_diagnostics().await;
            let query_diagnostics = related
                .iter()
                .find(|result| result.snapshot.uri == query_uri)
                .expect("open query diagnostics should be included");

            assert!(query_diagnostics.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == dsql_core::DiagnosticCode::FragmentTypeMismatch
            }));
        });
    }

    #[test]
    fn removing_fragment_reports_dependent_open_document_diagnostics() {
        block_on(async {
            let host = AnalysisHost::new();
            let fragment_uri = "file:///fragments.dsql".to_string();
            let query_uri = "file:///query.dsql".to_string();

            host.open_document(
                fragment_uri.clone(),
                1,
                "fragment UserFields on users { id }".to_string(),
            )
            .await;
            host.open_document(
                query_uri.clone(),
                1,
                "query Q { users { ...UserFields } }".to_string(),
            )
            .await;

            host.replace_document(
                fragment_uri.clone(),
                2,
                "query Empty { users { id } }".to_string(),
            )
            .await;
            let related = host.open_document_diagnostics().await;
            let query_diagnostics = related
                .iter()
                .find(|result| result.snapshot.uri == query_uri)
                .expect("open query diagnostics should be included");

            assert!(query_diagnostics.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == dsql_core::DiagnosticCode::UnknownFragment
            }));
        });
    }

    #[test]
    fn query_plan_uses_fragment_from_another_file() {
        block_on(async {
            let host = AnalysisHost::new();
            let fragment_uri = "file:///fragments.dsql".to_string();
            let query_uri = "file:///query.dsql".to_string();

            host.open_document(
                fragment_uri,
                1,
                "fragment UserFields on users { id name }".to_string(),
            )
            .await;
            let query = host
                .open_document(
                    query_uri.clone(),
                    1,
                    "query Q { users { ...UserFields } }".to_string(),
                )
                .await;
            assert!(
                query.diagnostics.is_empty(),
                "query diagnostics: {:?}",
                query.diagnostics
            );

            let diagnostics = host
                .document_diagnostics(&query_uri)
                .await
                .expect("query diagnostics should be available");
            let analysis = host
                .analyze(diagnostics.snapshot.file)
                .await
                .expect("query analysis should be available");

            assert!(analysis.plan.diagnostics.is_empty());
            assert_eq!(analysis.plan.queries.len(), 1);
            assert_eq!(analysis.plan.queries[0].selections.items.len(), 2);
        });
    }

    fn fragment_scope_names(scope: &completion::CompletionScope) -> Vec<&str> {
        scope
            .fragments
            .iter()
            .map(|fragment| fragment.key.name.as_str())
            .collect()
    }
}
