//! Editor services: hover answers arrive request/response through bound
//! entities, ranked by candidate priority; go-to-definition follows spread
//! resolutions to the fragment's name span.

use bowl::{Bowl, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::facts::VariablesDemand;
use dsql_core::register_language;
use dsql_core::service::{DefinitionRequest, DefinitionTarget, HoverInfo, HoverRequest, Position};
use dsql_core::source::{FilePath, insert_source};
use futures::executor::block_on;

use crate::{fixture, imdb_catalog};

const FIXTURE: &str = "valid/imdb-fragment-spread.dsql";

async fn service_bowl() -> Bowl {
    let bowl = Bowl::new();
    register_language(&bowl).await;
    insert_catalog(&bowl, imdb_catalog()).await;
    insert_source(&bowl, FIXTURE, &fixture(FIXTURE)).await;
    bowl
}

async fn hover(bowl: &Bowl, offset: usize) -> String {
    bowl.insert((
        HoverRequest,
        FilePath(FIXTURE.to_string()),
        Position { offset },
    ))
    .await
    .bind()
    .take::<HoverInfo>()
    .await
    .expect("hover requests are always answered")
    .0
    .clone()
}

#[test]
fn hover_answers_by_construct() {
    block_on(async {
        let bowl = service_bowl().await;
        let source = fixture(FIXTURE);

        let fragment_name = source.find("TitleFields").expect("fixture text");
        let root_field = source.find("title(").expect("fixture text");
        let column = source.find("production_year").expect("fixture text");
        let spread = source.find("...TitleFields").expect("fixture text") + "...".len();
        let relation = source.find("kind_type").expect("fixture text");

        insta::assert_snapshot!(format!(
            "fragment name: {}\nroot field: {}\ncolumn: {}\nspread: {}\nrelation: {}",
            hover(&bowl, fragment_name).await,
            hover(&bowl, root_field).await,
            hover(&bowl, column).await,
            hover(&bowl, spread).await,
            hover(&bowl, relation).await,
        ));
    });
}

#[test]
fn hover_on_variables_reports_bindings() {
    block_on(async {
        let bowl = Bowl::new();
        register_language(&bowl).await;
        insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
        bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
            .await;
        let source = "query Q {\n  users(where .name like $$search) {\n    id\n  }\n}\n";
        insert_source(&bowl, "vars.dsql", source).await;

        let offset = source.find("$$search").expect("test text") + "$$".len();
        let info = bowl
            .insert((
                HoverRequest,
                FilePath("vars.dsql".to_string()),
                Position { offset },
            ))
            .await
            .bind()
            .take::<HoverInfo>()
            .await
            .expect("hover answered")
            .0
            .clone();

        insta::assert_snapshot!(info);
    });
}

#[test]
fn hover_on_unknown_file_reports_it() {
    block_on(async {
        let bowl = service_bowl().await;
        let info = bowl
            .insert((
                HoverRequest,
                FilePath("missing.dsql".to_string()),
                Position { offset: 0 },
            ))
            .await
            .bind()
            .take::<HoverInfo>()
            .await
            .expect("hover answered");
        assert_eq!(info.0, "unknown file");
    });
}

#[test]
fn goto_definition_follows_spreads() {
    block_on(async {
        let bowl = service_bowl().await;
        let source = fixture(FIXTURE);

        let spread = source.find("...TitleFields").expect("fixture text") + "...".len();
        let target = bowl
            .insert((
                DefinitionRequest,
                FilePath(FIXTURE.to_string()),
                Position { offset: spread },
            ))
            .await
            .bind()
            .take::<DefinitionTarget>()
            .await
            .expect("definition answered");

        let name_start = source.find("TitleFields").expect("fixture text");
        assert_eq!(target.span.start, name_start);
        assert_eq!(target.span.end, name_start + "TitleFields".len());
    });
}

#[test]
fn semantic_tokens_classify_by_resolution() {
    use dsql_core::service::{SemanticTokens, SemanticTokensRequest};

    block_on(async {
        let bowl = service_bowl().await;
        let source = fixture(FIXTURE);

        let tokens = bowl
            .insert((SemanticTokensRequest, FilePath(FIXTURE.to_string())))
            .await
            .bind()
            .take::<SemanticTokens>()
            .await
            .expect("semantic tokens answered");

        let rendered: Vec<String> = tokens
            .0
            .iter()
            .map(|token| {
                format!(
                    "{:?} {}..{} `{}`",
                    token.kind,
                    token.span.start,
                    token.span.end,
                    &source[token.span.start..token.span.end],
                )
            })
            .collect();
        insta::assert_snapshot!(rendered.join("\n"));
    });
}

#[test]
fn semantic_tokens_for_unknown_file_are_empty() {
    use dsql_core::service::{SemanticTokens, SemanticTokensRequest};

    block_on(async {
        let bowl = service_bowl().await;
        let tokens = bowl
            .insert((SemanticTokensRequest, FilePath("missing.dsql".to_string())))
            .await
            .bind()
            .take::<SemanticTokens>()
            .await
            .expect("semantic tokens answered");
        assert!(tokens.0.is_empty());
    });
}
