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
}
