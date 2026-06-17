mod analysis;
mod completion;
mod cursor;
mod db;
mod definition;
mod definitions;
mod document;
mod hover;
mod project;
mod provider;
mod semantic_tokens;

pub use completion::{CompletionItem, CompletionKind};
pub use db::{AnalysisResult, ParsedFile};
pub use definition::{CatalogDefinition, DefinitionResult, SourceDefinition, SourceDefinitionKind};
pub use definitions::{
    DefinitionId, DefinitionIndex, FragmentRoot, FragmentRootInner, QueryRoot, QueryRootInner,
    Root, RootDefinition, RootDefinitionBucket, SourceRegionId,
};
pub use document::{
    DocumentDiagnostics, DocumentFormat, DocumentSnapshot, RevisionId, SourceUnitId, TextEdit,
    TextEditRange, TextPosition,
};
pub use hover::HoverInfo;
pub use project::{
    AnalysisContext, AnalysisContextId, DocumentBundle, PhysicalDocument, PhysicalDocumentId,
    PresentedDiagnostic, ProjectContextSource, ProjectDiagnostic, ProjectGenerationContext,
    ProjectGenerationDefinition, ProjectGenerationModel, ProjectHost, ProjectSourceRegion,
    ProjectSourceScope, SourceDb, SourceEntry, SourcePosition, SourceResidency,
};
pub use provider::{CatalogProvider, HardcodedCatalogProvider};
pub use semantic_tokens::{DocumentSemanticTokens, SemanticTokenInfo, SemanticTokenKind};

fn range_contains(range: dsql_core::TextRange, byte: usize) -> bool {
    (range.start as usize) <= byte && byte <= (range.end as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsql_core::{Catalog, ParseResult, SourceFile, parse_source};

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
}
