mod analysis;
mod completion;
mod db;
mod document;
mod host;
mod hover;
mod provider;
mod semantic_tokens;

pub use analysis::{analyze_source, collect_diagnostics};
pub use completion::{CompletionItem, CompletionKind};
pub use db::{AnalysisResult, ParsedFile};
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
    use dsql_core::{Catalog, SourceFile, parse_source};
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

    #[test]
    fn completions_include_exact_relation_names() {
        let source = "query Q { posts {  } }";
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let byte = source.find("  }").unwrap() + 1;

        let completions = completion::completions_at(&source_file, &catalog, byte);

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
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let byte = source.find("  }").unwrap() + 1;

        let completions = completion::completions_at(&source_file, &catalog, byte);

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
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let completion_byte = source.find("  posts").unwrap() + 1;
        let hover_byte = source.find("title").unwrap();

        let completions = completion::completions_at(&source_file, &catalog, completion_byte);
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
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let completion_byte = source.find("public.posts").unwrap() - 1;
        let hover_byte = source.find("public.posts").unwrap();

        let completions = completion::completions_at(&source_file, &catalog, completion_byte);
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
            assert_eq!(analysis.plan.queries[0].selections.projections.len(), 2);
        });
    }
}
