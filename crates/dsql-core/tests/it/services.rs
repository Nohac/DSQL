//! Editor services: hover answers arrive request/response through bound
//! entities, ranked by candidate priority; go-to-definition follows spread
//! resolutions to the fragment's name span.

use bowl::{Bowl, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::facts::VariablesDemand;
use dsql_core::language_bowl;
use dsql_core::service::{DefinitionRequest, DefinitionTarget, HoverInfo, HoverRequest, Position};
use dsql_core::source::{FilePath, insert_source};
use futures::executor::block_on;

use crate::{fixture, imdb_catalog};

const FIXTURE: &str = "valid/imdb-fragment-spread.dsql";

async fn service_bowl() -> Bowl {
    let bowl = language_bowl().await;
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
    .text
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
        let bowl = language_bowl().await;
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
            .text
            .clone();

        insta::assert_snapshot!(info);
    });
}

#[test]
fn query_definition_hover_reports_inferred_variables() {
    block_on(async {
        let source = concat!(
            "query MovieDetailPageQuery {\n",
            "  users(\n",
            "    where .id == $$movieId\n",
            "    order by created_at $$direction\n",
            "    limit $count\n",
            "  ) {\n",
            "    id\n",
            "  }\n",
            "}\n",
            "query NoVariables {\n",
            "  users(limit 1) {\n",
            "    id\n",
            "  }\n",
            "}\n",
        );

        let bowl = language_bowl().await;
        insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
        bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
            .await;
        insert_source(&bowl, FIXTURE, source).await;

        let query = source.find("MovieDetailPageQuery").expect("test text");
        let no_variables = source.find("NoVariables").expect("test text");
        let with_variables = hover(&bowl, query).await;
        let without_variables = hover(&bowl, no_variables).await;

        let bowl_without_demand = language_bowl().await;
        insert_catalog(
            &bowl_without_demand,
            dsql_core::catalog::Catalog::hardcoded(),
        )
        .await;
        insert_source(&bowl_without_demand, FIXTURE, source).await;
        let without_demand = hover(&bowl_without_demand, query).await;

        insta::assert_snapshot!(format!(
            "with variables:\n{with_variables}\n\nwithout variables:\n{without_variables}\n\nwithout variable demand:\n{without_demand}"
        ));
    });
}

#[test]
fn clause_fields_hover_from_semantic_resolutions() {
    block_on(async {
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        let source = concat!(
            "query TopRated {\n",
            "  movie_info_idx(\n",
            "    where .info_type_id == 101\n",
            "      and .title.kind_id == 1\n",
            "      and .title.movie_info_idx.info_type_id == 100\n",
            "    order by info desc, id asc\n",
            "    limit 16\n",
            "  ) {\n",
            "    id\n",
            "  }\n",
            "}\n",
        );
        insert_source(&bowl, FIXTURE, source).await;

        let direct_column = source.find(".info_type_id").expect("test text") + 1;
        let first_relation = source.find(".title.kind_id").expect("test text") + 1;
        let first_terminal = source.find("kind_id == 1").expect("test text");
        let nested_relation =
            source.find(".title.movie_info_idx").expect("test text") + ".title.".len();
        let nested_terminal = source.rfind("info_type_id").expect("test text");
        let order_info = source.find("order by info").expect("test text") + "order by ".len();
        let order_id = source.find("info desc, id").expect("test text") + "info desc, ".len();

        insta::assert_snapshot!(format!(
            "direct column: {}\nfirst relation: {}\nfirst terminal: {}\nnested relation: {}\nnested terminal: {}\norder info: {}\norder id: {}",
            hover(&bowl, direct_column).await,
            hover(&bowl, first_relation).await,
            hover(&bowl, first_terminal).await,
            hover(&bowl, nested_relation).await,
            hover(&bowl, nested_terminal).await,
            hover(&bowl, order_info).await,
            hover(&bowl, order_id).await,
        ));
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
        assert_eq!(info.text, "unknown file");
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
        assert!(
            matches!(target.as_ref(), DefinitionTarget::Source { .. }),
            "spread definition must target dsql source"
        );
        if let DefinitionTarget::Source { span, .. } = target.as_ref() {
            assert_eq!(span.start, name_start);
            assert_eq!(span.end, name_start + "TitleFields".len());
        }
    });
}

#[test]
fn semantic_tokens_classify_by_resolution() {
    use dsql_core::service::semantic_tokens;

    block_on(async {
        let bowl = service_bowl().await;
        let source = fixture(FIXTURE);

        let tokens = semantic_tokens(&bowl, FIXTURE).await;

        let rendered: Vec<String> = tokens
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
    use dsql_core::service::semantic_tokens;

    block_on(async {
        let bowl = service_bowl().await;
        let tokens = semantic_tokens(&bowl, "missing.dsql").await;
        assert!(tokens.is_empty());
    });
}

/// The editor stamps `OpenBuffer` on a file it opens; that must not
/// disturb the file's derived facts (the marker is untracked — a tracked
/// insert would retire every fact anchored to the file with nothing
/// re-deriving them).
#[test]
fn hover_survives_opening_the_buffer() {
    use dsql_core::source::{OpenBuffer, SourceText, insert_source};

    block_on(async {
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        let source = fixture(FIXTURE);
        let file = insert_source(&bowl, FIXTURE, &source).await;

        let offset = source.find("production_year").expect("fixture text");
        let before = hover(&bowl, offset).await;
        assert!(before.contains("column"), "hover answers before: {before}");

        // The LSP `didOpen` flow: replace the text wholesale (identical
        // content) and stamp the open-buffer marker.
        let sources = bowl
            .scoop::<bowl::Query<(bowl::Entity, bowl::Mut<SourceText>)>>()
            .await;
        for (entity, text) in sources.collect() {
            if entity == file {
                let content = source.clone();
                text.with_latest(move |text| text.set_text(&content)).await;
            }
        }
        bowl.entity(file).insert((OpenBuffer,)).await;

        let after = hover(&bowl, offset).await;
        assert_eq!(after, before, "opening the buffer must not lose facts");
    });
}
