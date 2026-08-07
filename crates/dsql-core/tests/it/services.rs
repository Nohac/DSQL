//! Editor services: hover answers arrive request/response through bound
//! entities, ranked by candidate priority; go-to-definition follows spread
//! resolutions to the fragment's name span.

use std::collections::BTreeSet;

use bowl::{Bowl, Singleton};
use dsql_core::catalog::{
    ColumnMetadata, DataType, DatabaseMetadata, ObjectType, SchemaMetadata, TableMetadata, TypeKey,
    TypeMetadata, TypeStructureMetadata, insert_catalog,
};
use dsql_core::facts::{VariablesDemand, arm_editor_demands};
use dsql_core::language_bowl;
use dsql_core::service::{DefinitionRequest, DefinitionTarget, HoverInfo, HoverRequest, Position};
use dsql_core::source::{FilePath, insert_source};

use crate::{fixture, imdb_catalog, set_source_text};

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

#[tokio::test]
async fn hover_answers_by_construct() {
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
}

#[tokio::test]
async fn catalog_descriptions_reach_table_column_and_relation_hover() {
    let mut catalog = dsql_core::catalog::Catalog::hardcoded();
    catalog.tables[0].description = Some("Application accounts.".to_string());
    catalog.tables[1].description = Some("Articles published by an account.".to_string());
    catalog.columns[1].description = Some("The account's public display name.".to_string());
    catalog.columns[5].description = Some("The article headline.".to_string());

    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    let source = indoc::indoc! {r#"
        query Documented {
          public::users(where .name like "A%") {
            name
            posts {
              title
            }
          }
        }
    "#};
    insert_source(&bowl, FIXTURE, source).await;

    let root = source.find("public::users").expect("test text") + "public::".len();
    let predicate = source.find(".name").expect("test text") + 1;
    let column = source.find("    name").expect("test text") + 4;
    let relation = source.find("posts").expect("test text");
    let related_column = source.find("title").expect("test text");

    insta::assert_snapshot!(format!(
        "root:\n{}\n\npredicate:\n{}\n\ncolumn:\n{}\n\nrelation:\n{}\n\nrelated column:\n{}",
        hover(&bowl, root).await,
        hover(&bowl, predicate).await,
        hover(&bowl, column).await,
        hover(&bowl, relation).await,
        hover(&bowl, related_column).await,
    ));
}

#[tokio::test]
async fn column_hover_uses_the_provider_formatted_type() {
    let bowl = language_bowl().await;
    let catalog = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![TableMetadata {
                schema: "public".to_string(),
                name: "records".to_string(),
                object_type: ObjectType::Table,
                description: None,
                columns: vec![ColumnMetadata {
                    name: "label".to_string(),
                    description: None,
                    provider_type: TypeKey::new("pg_catalog", "varchar"),
                    formatted_type: Some("character varying(20)".to_string()),
                    type_modifier: Some(24),
                    database_type: "varchar".to_string(),
                    data_type: DataType::Text,
                    not_null: true,
                }],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: vec![TypeMetadata {
            internal_type: "varchar".to_string(),
            readable_type: "character varying".to_string(),
            schema: "pg_catalog".to_string(),
            structure: TypeStructureMetadata::scalar(),
            provider: None,
            operations: BTreeSet::new(),
        }],
    }
    .to_catalog()
    .expect("formatted provider type catalog builds");
    insert_catalog(&bowl, catalog).await;
    let source = "query Q { records { label } }";
    insert_source(&bowl, FIXTURE, source).await;

    insta::assert_snapshot!(hover(&bowl, source.find("label").expect("test text")).await);
}

#[tokio::test]
async fn hover_on_variables_reports_bindings() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    let source = "query Q {\n  public::users(where .name like %search) {\n    id\n  }\n}\n";
    insert_source(&bowl, "vars.dsql", source).await;

    let offset = source.find("%search").expect("test text") + "%".len();
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
}

#[tokio::test]
async fn hover_on_bounded_dynamic_variables_reports_their_roles() {
    let source = indoc::indoc! {r#"
        query Q {
          public::users(
            where %search on selected
              and %indexed_search on indexed
              and %selected_indexed_search on selected_indexed
            order by %order on selected
          ) {
            id
            name
          }
        }
    "#};
    let bowl = language_bowl().await;
    insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    insert_source(&bowl, FIXTURE, source).await;

    let variables = [
        "search",
        "indexed_search",
        "selected_indexed_search",
        "order",
    ]
    .map(|name| {
        let offset = source
            .find(&format!("%{name}"))
            .expect("test variable exists")
            + "%".len();
        (name, offset)
    });
    let mut rendered = Vec::new();
    for (name, offset) in variables {
        rendered.push(format!("{name}: {}", hover(&bowl, offset).await));
    }

    insta::assert_snapshot!(rendered.join("\n"));
}

#[tokio::test]
async fn query_definition_hover_reports_inferred_variables() {
    let source = indoc::indoc! {r#"
        query MovieDetailPageQuery(%movieId? = null %direction = "desc" $count = 10) {
          public::users(
            where .id == %movieId
            order by created_at %direction
            limit $count
          ) {
            id
          }
        }
        query NoVariables {
          public::users(limit 1) {
            id
          }
        }
        query DynamicReadingSearch(
          %selected_search = {}
          %indexed_search = {}
          %searchable_search = {}
          %selected_order = []
          %selected_indexed_order = []
        ) {
          public::users(
            where %selected_search on selected
              and %indexed_search on indexed
              and %searchable_search on searchable
            order by %selected_order on selected,
              %selected_indexed_order on selected_indexed
          ) {
            id
            display: name
          }
        }
    "#};

    let bowl = language_bowl().await;
    insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    insert_source(&bowl, FIXTURE, source).await;

    let query = source.find("MovieDetailPageQuery").expect("test text");
    let no_variables = source.find("NoVariables").expect("test text");
    let dynamic = source.find("DynamicReadingSearch").expect("test text");
    let with_variables = hover(&bowl, query).await;
    let without_variables = hover(&bowl, no_variables).await;
    let with_dynamic_shape = hover(&bowl, dynamic).await;

    let bowl_without_demand = language_bowl().await;
    insert_catalog(
        &bowl_without_demand,
        dsql_core::catalog::Catalog::hardcoded(),
    )
    .await;
    insert_source(&bowl_without_demand, FIXTURE, source).await;
    let without_demand = hover(&bowl_without_demand, query).await;

    insta::assert_snapshot!(format!(
        "with variables:\n{with_variables}\n\nwith dynamic shape:\n{with_dynamic_shape}\n\nwithout variables:\n{without_variables}\n\nwithout variable demand:\n{without_demand}"
    ));
}

#[tokio::test]
async fn query_definition_hover_rederives_dynamic_shapes_after_edits() {
    let initial = indoc::indoc! {r#"
        query DynamicReadingSearch(%search = {} %order = []) {
          public::users(
            where %search on selected
            order by %order on selected
          ) {
            id
          }
        }
    "#};
    let updated = indoc::indoc! {r#"
        query DynamicReadingSearch(%search = {} %order = []) {
          public::users(
            where %search on selected
            order by %order on selected
          ) {
            contact: email
            recorded: created_at
          }
        }
    "#};

    let bowl = language_bowl().await;
    insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    let file = insert_source(&bowl, FIXTURE, initial).await;
    let before = hover(
        &bowl,
        initial.find("DynamicReadingSearch").expect("test text"),
    )
    .await;

    set_source_text(&bowl, file, updated).await;
    let after = hover(
        &bowl,
        updated.find("DynamicReadingSearch").expect("test text"),
    )
    .await;

    insta::assert_snapshot!(format!("before:\n{before}\n\nafter:\n{after}"));
}

#[tokio::test]
async fn clause_fields_hover_from_semantic_resolutions() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let source = indoc::indoc! {r#"
        query TopRated {
          movie_info_idx(
            where .info_type_id == 101
              and .title.kind_id == 1
              and .title.movie_info_idx.info_type_id == 100
            order by info desc, id asc
            limit 16
          ) {
            id
          }
        }
    "#};
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
}

#[tokio::test]
async fn aggregate_fields_hover_and_tokenize_from_semantic_resolutions() {
    use dsql_core::service::semantic_tokens;

    let bowl = language_bowl().await;
    insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
    let source = indoc::indoc! {r#"
        query Stats {
          user_stats: public::users | aggregate {
            count
            first_name: min .name
          }
          user_groups: public::users | aggregate by name_group: .name {
            count
          }
        }
    "#};
    insert_source(&bowl, FIXTURE, source).await;

    let count = source.find("count").expect("test text");
    let alias = source.find("first_name").expect("test text");
    let function = source.find("min .name").expect("test text");
    let operand = source.find(".name").expect("test text") + 1;
    let group_alias = source.find("name_group").expect("test text");
    let group_column = source.rfind(".name").expect("test text") + 1;
    let tokens = semantic_tokens(&bowl, FIXTURE)
        .await
        .into_iter()
        .map(|token| {
            format!(
                "{:?} {}..{} `{}`",
                token.kind,
                token.span.start,
                token.span.end,
                &source[token.span.start..token.span.end],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    insta::assert_snapshot!(format!(
        "count: {}\nalias: {}\nfunction: {}\noperand: {}\ngroup alias: {}\ngroup column: {}\n\ntokens:\n{tokens}",
        hover(&bowl, count).await,
        hover(&bowl, alias).await,
        hover(&bowl, function).await,
        hover(&bowl, operand).await,
        hover(&bowl, group_alias).await,
        hover(&bowl, group_column).await,
    ));
}

#[tokio::test]
async fn aggregate_predicates_hover_and_tokenize_from_clause_resolutions() {
    use dsql_core::service::semantic_tokens;

    let bowl = language_bowl().await;
    insert_catalog(&bowl, dsql_core::catalog::Catalog::hardcoded()).await;
    let source = indoc::indoc! {r#"
        query Filtered {
          public::users(
            where .posts | exists
              and (.posts | min .title) like "A%"
            limit 1
          ) { id }
        }
    "#};
    insert_source(&bowl, FIXTURE, source).await;

    let first_relation = source.find(".posts").expect("test text") + 1;
    let exists = source.find("exists").expect("test text");
    let second_relation = source.rfind(".posts").expect("test text") + 1;
    let min = source.find("min .title").expect("test text");
    let operand = source.find(".title").expect("test text") + 1;
    let tokens = semantic_tokens(&bowl, FIXTURE)
        .await
        .into_iter()
        .map(|token| {
            format!(
                "{:?} {}..{} `{}`",
                token.kind,
                token.span.start,
                token.span.end,
                &source[token.span.start..token.span.end],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    insta::assert_snapshot!(format!(
        "first relation: {}\nexists: {}\nsecond relation: {}\nmin: {}\noperand: {}\n\ntokens:\n{tokens}",
        hover(&bowl, first_relation).await,
        hover(&bowl, exists).await,
        hover(&bowl, second_relation).await,
        hover(&bowl, min).await,
        hover(&bowl, operand).await,
    ));
}

#[tokio::test]
async fn hover_on_unknown_file_reports_it() {
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
}

#[tokio::test]
async fn goto_definition_follows_spreads() {
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
}

#[tokio::test]
async fn spread_navigation_follows_span_changes_without_semantic_resolution() {
    const FRAGMENT_PATH: &str = "fragments.dsql";
    const QUERY_PATH: &str = "query.dsql";
    const FRAGMENT: &str = "fragment TitleBits on title { id }\n";
    const QUERY: &str = "query Titles { title(limit 1) { ...TitleBits } }\n";

    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let fragment = insert_source(&bowl, FRAGMENT_PATH, FRAGMENT).await;
    insert_source(&bowl, QUERY_PATH, QUERY).await;

    async fn definition_at(bowl: &Bowl) -> (bowl::Entity, dsql_core::facts::Span) {
        let spread = QUERY.find("TitleBits").expect("query text");
        let target = bowl
            .insert((
                DefinitionRequest,
                FilePath(QUERY_PATH.to_string()),
                Position { offset: spread },
            ))
            .await
            .bind()
            .take::<DefinitionTarget>()
            .await
            .expect("definition answered");
        match target.as_ref() {
            DefinitionTarget::Source { file, span } => Some((*file, *span)),
            DefinitionTarget::Catalog(_) => None,
        }
        .expect("spread definition must target source")
    }

    async fn system_runs(bowl: &Bowl, suffix: &str) -> u64 {
        bowl.profile_all()
            .await
            .into_iter()
            .find(|entry| entry.name.ends_with(suffix))
            .map(|entry| entry.runs)
            .unwrap_or_default()
    }

    let (initial_file, initial_span) = definition_at(&bowl).await;
    let initial_binder_runs = system_runs(&bowl, "bind_visible_fragment_candidates").await;
    let initial_resolver_runs = system_runs(&bowl, "resolve_spreads").await;
    assert_eq!(initial_file, fragment);
    assert_eq!(initial_span.start, 9);

    set_source_text(&bowl, fragment, format!("\n{FRAGMENT}")).await;

    let (moved_file, moved_span) = definition_at(&bowl).await;
    assert_eq!(
        system_runs(&bowl, "bind_visible_fragment_candidates").await,
        initial_binder_runs,
        "navigation-only span changes must not wake candidate binding"
    );
    assert_eq!(
        system_runs(&bowl, "resolve_spreads").await,
        initial_resolver_runs,
        "navigation-only span changes must not wake semantic spread resolution"
    );
    assert_eq!(moved_file, fragment);
    assert_eq!(moved_span.start, initial_span.start + 1);
    assert_eq!(moved_span.end, initial_span.end + 1);
}

#[tokio::test]
async fn semantic_tokens_classify_by_resolution() {
    use dsql_core::service::semantic_tokens;

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
}

#[tokio::test]
async fn semantic_tokens_for_unknown_file_are_empty() {
    use dsql_core::service::semantic_tokens;

    let bowl = service_bowl().await;
    let tokens = semantic_tokens(&bowl, "missing.dsql").await;
    assert!(tokens.is_empty());
}

/// The editor stamps `OpenBuffer` on a file it opens; that must not
/// disturb the file's derived facts (the marker is untracked — a tracked
/// insert would retire every fact anchored to the file with nothing
/// re-deriving them).
#[tokio::test]
async fn hover_survives_opening_the_buffer() {
    use dsql_core::source::{OpenBuffer, insert_source};

    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let source = fixture(FIXTURE);
    let file = insert_source(&bowl, FIXTURE, &source).await;

    let offset = source.find("production_year").expect("fixture text");
    let before = hover(&bowl, offset).await;
    assert!(before.contains("column"), "hover answers before: {before}");

    // The LSP `didOpen` flow: replace the text wholesale (identical
    // content) and stamp the open-buffer marker.
    set_source_text(&bowl, file, source).await;
    bowl.entity(file).insert((OpenBuffer,)).await;

    let after = hover(&bowl, offset).await;
    assert_eq!(after, before, "opening the buffer must not lose facts");
}
