use dsql_core::{Catalog, SourceSnapshot, parse_source};
use dsql_frontend::{
    AnalysisHost, CatalogDefinition, CompletionKind, DefinitionResult, SourceDefinition,
    SourceDefinitionKind, TextPosition,
};
use insta::Settings;
use std::{collections::BTreeMap, fs, path::PathBuf};

#[tokio::test]
async fn imdb_lsp_fixture_snapshots() {
    let catalog = imdb_catalog();
    let fixture = fixture_root().join("queries/lsp/imdb-title-lsp.dsql");
    let source = fs::read_to_string(&fixture).unwrap();
    let uri = "file:///tests/queries/lsp/imdb-title-lsp.dsql".to_string();
    let host = AnalysisHost::new();
    host.set_catalog(catalog);
    host.open_document(uri.clone(), 1, source.clone()).await;

    let table_completion = host
        .completions(&uri, position_after(&source, "query LspTitle {\n  "))
        .await
        .unwrap();
    let title_completion = host
        .completions(&uri, position_after(&source, "  }\n    \n"))
        .await
        .unwrap();
    let nested_completion = host
        .completions(&uri, position_after(&source, "      \n"))
        .await
        .unwrap();
    let table_hover = host
        .hover(&uri, position_at(&source, "title(where"))
        .await
        .unwrap();
    let where_hover = host
        .hover(&uri, position_at(&source, "id >"))
        .await
        .unwrap();
    let relation_hover = host
        .hover(&uri, position_at(&source, "kind_type"))
        .await
        .unwrap();
    let semantic_tokens = host.semantic_tokens(&uri).await.unwrap();
    let parsed = parse_source(SourceSnapshot::from_string(source));

    snapshot(
        "lsp_table_completions",
        &format_completions(&table_completion),
    );
    snapshot(
        "lsp_title_completions",
        &format_completions(&title_completion),
    );
    snapshot(
        "lsp_nested_completions",
        &format_completions(&nested_completion),
    );
    snapshot("lsp_table_hover", &table_hover.markdown);
    snapshot("lsp_where_hover", &where_hover.markdown);
    snapshot("lsp_relation_hover", &relation_hover.markdown);
    snapshot(
        "lsp_semantic_tokens",
        &semantic_tokens
            .tokens
            .iter()
            .map(|token| {
                format!(
                    "{:?} {}",
                    token.kind,
                    parsed.source.text(token.range).as_ref()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[tokio::test]
async fn imdb_keyword_and_operator_completion_contexts() {
    let (source, markers) = marked_source("queries/lsp/imdb-completion-contexts.dsql");
    let uri = "file:///tests/queries/lsp/imdb-completion-contexts.dsql".to_string();
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    host.open_document(uri.clone(), 1, source.clone()).await;

    let document_root =
        completions_at_marker(&host, &uri, &source, &markers, "document_root").await;
    let fragment_on = completions_at_marker(&host, &uri, &source, &markers, "fragment_on").await;
    let fragment_type =
        completions_at_marker(&host, &uri, &source, &markers, "fragment_type").await;
    let clause_keywords =
        completions_at_marker(&host, &uri, &source, &markers, "clause_keywords").await;
    let int_operators = completions_at_marker(&host, &uri, &source, &markers, "int_operator").await;
    let order_columns = completions_at_marker(&host, &uri, &source, &markers, "order_column").await;
    let sort_directions =
        completions_at_marker(&host, &uri, &source, &markers, "sort_direction").await;
    let text_operators =
        completions_at_marker(&host, &uri, &source, &markers, "text_operator").await;
    let clause_prefix =
        completions_at_marker(&host, &uri, &source, &markers, "clause_prefix").await;
    let where_scope = completions_at_marker(&host, &uri, &source, &markers, "where_scope").await;
    let where_columns = completions_at_marker(&host, &uri, &source, &markers, "where_column").await;
    let where_relations =
        completions_at_marker(&host, &uri, &source, &markers, "where_relation").await;
    let where_relation_selectors =
        completions_at_marker(&host, &uri, &source, &markers, "where_relation_selector").await;
    let where_relation_columns =
        completions_at_marker(&host, &uri, &source, &markers, "where_relation_column").await;
    let where_relation_text_operators = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "where_relation_text_operator",
    )
    .await;
    let where_root_columns =
        completions_at_marker(&host, &uri, &source, &markers, "where_root_column").await;
    let where_parent_columns =
        completions_at_marker(&host, &uri, &source, &markers, "where_parent_column").await;
    let where_completed_predicate =
        completions_at_marker(&host, &uri, &source, &markers, "where_completed_predicate").await;
    let order_columns_after_where =
        completions_at_marker(&host, &uri, &source, &markers, "order_column_after_where").await;
    let sort_direction_after_where =
        completions_at_marker(&host, &uri, &source, &markers, "sort_direction_after_where").await;
    let where_nested_relation_selector = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "where_nested_relation_selector",
    )
    .await;
    let where_rhs_columns =
        completions_at_marker(&host, &uri, &source, &markers, "where_rhs_column").await;
    let relation_clause_exact_nested = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "relation_clause_exact_nested",
    )
    .await;
    let scalar_clause =
        completions_at_marker(&host, &uri, &source, &markers, "scalar_clause").await;
    let unknown_clause =
        completions_at_marker(&host, &uri, &source, &markers, "unknown_clause").await;
    let movie_info_body_after_clause = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "movie_info_body_after_clause",
    )
    .await;
    let fragment_spread =
        completions_at_marker(&host, &uri, &source, &markers, "fragment_spread").await;
    let title_body_after_clause =
        completions_at_marker(&host, &uri, &source, &markers, "title_body_after_clause").await;
    let relation_path_clause_recovery = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "relation_path_clause_recovery",
    )
    .await;
    let relation_path_closed_clause_recovery = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "relation_path_closed_clause_recovery",
    )
    .await;
    let relation_path_after_fragment = completions_at_marker(
        &host,
        &uri,
        &source,
        &markers,
        "relation_path_after_fragment",
    )
    .await;

    assert_completion_labels("document root", &document_root, &["query", "fragment"]);
    assert_no_completion_labels(
        "document root",
        &document_root,
        &["id", "movie_info", "title"],
    );
    assert_completion_labels("fragment on", &fragment_on, &["on"]);
    assert_no_completion_labels("fragment on", &fragment_on, &["id", "movie_info"]);
    assert_completion_labels("fragment type", &fragment_type, &["title", "movie_info"]);
    assert_no_completion_labels("fragment type", &fragment_type, &["id", "on"]);
    assert_completion_labels(
        "clause keywords",
        &clause_keywords,
        &["where", "order by", "limit", "offset"],
    );
    assert_completion_labels(
        "int operators",
        &int_operators,
        &["==", "!=", ">", ">=", "<", "<="],
    );
    assert_completion_labels(
        "order columns",
        &order_columns,
        &["id", "title", "production_year"],
    );
    assert_completion_labels("sort directions", &sort_directions, &["asc", "desc"]);
    assert_completion_labels("text operators", &text_operators, &["==", "!="]);
    assert_completion_labels("clause prefix", &clause_prefix, &["where"]);
    assert_completion_labels("where scope", &where_scope, &[".", "~", ".."]);
    assert_no_completion_labels("where scope", &where_scope, &["id", "info", "movie_id"]);
    assert_completion_labels("where columns", &where_columns, &["id", "info", "movie_id"]);
    assert_completion_labels(
        "where relations",
        &where_relations,
        &["aka_title::movie_id", "aka_title::episode_of_id"],
    );
    assert_completion_labels(
        "where relation selectors",
        &where_relation_selectors,
        &["movie_id", "episode_of_id"],
    );
    assert_no_completion_labels(
        "where relation selectors",
        &where_relation_selectors,
        &["aka_title::movie_id", "title", "id"],
    );
    assert_completion_labels(
        "where relation columns",
        &where_relation_columns,
        &["id", "title", "episode_nr"],
    );
    assert_no_completion_labels(
        "where relation columns",
        &where_relation_columns,
        &["aka_title::movie_id", "aka_title::episode_of_id"],
    );
    assert_completion_labels(
        "where relation text operators",
        &where_relation_text_operators,
        &["==", "!=", "like"],
    );
    assert_completion_labels(
        "where root columns",
        &where_root_columns,
        &["id", "info", "movie_id"],
    );
    assert_no_completion_labels(
        "where root columns",
        &where_root_columns,
        &["episode_nr", "kind_id"],
    );
    assert_completion_labels(
        "where parent columns",
        &where_parent_columns,
        &["id", "info", "movie_id"],
    );
    assert_no_completion_labels(
        "where parent columns",
        &where_parent_columns,
        &["episode_nr", "kind_id"],
    );
    assert_completion_labels(
        "where completed predicate",
        &where_completed_predicate,
        &["order by", "limit", "offset"],
    );
    assert_no_completion_labels(
        "where completed predicate",
        &where_completed_predicate,
        &["==", "!=", "like"],
    );
    assert_completion_labels(
        "order columns after where",
        &order_columns_after_where,
        &["id", "info", "movie_id"],
    );
    assert_completion_labels(
        "sort direction after where",
        &sort_direction_after_where,
        &["asc", "desc"],
    );
    assert_completion_labels(
        "where nested relation selector",
        &where_nested_relation_selector,
        &["movie_id", "episode_of_id"],
    );
    assert_no_completion_labels(
        "where nested relation selector",
        &where_nested_relation_selector,
        &["aka_title::movie_id", "title", "id"],
    );
    assert_completion_labels(
        "where rhs columns",
        &where_rhs_columns,
        &["id", "info", "movie_id"],
    );
    assert_no_completion_labels(
        "where rhs columns",
        &where_rhs_columns,
        &["episode_nr", "kind_id", "==", "!="],
    );
    assert_completion_labels(
        "relation clause exact nested",
        &relation_clause_exact_nested,
        &["where", "order by", "limit", "offset"],
    );
    assert_no_completion_labels(
        "relation clause exact nested",
        &relation_clause_exact_nested,
        &[
            "id",
            "movie_id",
            "cast_info",
            "==",
            "!=",
            ">",
            ">=",
            "<",
            "<=",
        ],
    );
    assert!(
        scalar_clause.is_empty(),
        "scalar clause should not return completions; got {:?}",
        scalar_clause
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        unknown_clause.is_empty(),
        "unknown clause should not return completions; got {:?}",
        unknown_clause
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>()
    );
    assert_completion_labels(
        "movie info body after clause",
        &movie_info_body_after_clause,
        &["id", "movie_id", "note", "title", "info_type"],
    );
    assert_no_completion_labels(
        "movie info body after clause",
        &movie_info_body_after_clause,
        &["==", "!=", ">", ">=", "<", "<="],
    );
    assert_completion_labels("fragment spread", &fragment_spread, &["MoviesInfo"]);
    assert_no_completion_labels(
        "fragment spread",
        &fragment_spread,
        &["id", "note", "movie_id", "title"],
    );
    assert_completion_labels(
        "title body after clause",
        &title_body_after_clause,
        &["id", "title", "episode_nr", "cast_info"],
    );
    assert_no_completion_labels(
        "title body after clause",
        &title_body_after_clause,
        &["==", "!=", ">", ">=", "<", "<="],
    );
    assert_no_completion_labels(
        "clause prefix",
        &clause_prefix,
        &["movie_keyword", "keyword"],
    );
    assert_completion_labels(
        "relation path clause recovery",
        &relation_path_clause_recovery,
        &["where", "order by", "limit", "offset"],
    );
    assert_no_completion_labels(
        "relation path clause recovery",
        &relation_path_clause_recovery,
        &["id", "aka_title::movie_id", "aka_title::episode_of_id"],
    );
    assert_completion_labels(
        "relation path closed clause recovery",
        &relation_path_closed_clause_recovery,
        &["where", "order by", "limit", "offset"],
    );
    assert_no_completion_labels(
        "relation path closed clause recovery",
        &relation_path_closed_clause_recovery,
        &["id", "aka_title::movie_id", "aka_title::episode_of_id"],
    );
    assert_completion_labels(
        "relation path after fragment",
        &relation_path_after_fragment,
        &["where", "order by", "limit", "offset"],
    );
    assert_no_completion_labels(
        "relation path after fragment",
        &relation_path_after_fragment,
        &["id", "aka_title::movie_id", "aka_title::episode_of_id"],
    );

    snapshot(
        "lsp_completion_contexts",
        &[
            ("document_root", &document_root),
            ("fragment_on", &fragment_on),
            ("fragment_type", &fragment_type),
            ("clause_keywords", &clause_keywords),
            ("int_operator", &int_operators),
            ("text_operator", &text_operators),
            ("where_scope", &where_scope),
            ("where_column", &where_columns),
            ("where_relation", &where_relations),
            ("where_relation_selector", &where_relation_selectors),
            ("where_relation_column", &where_relation_columns),
            (
                "where_relation_text_operator",
                &where_relation_text_operators,
            ),
            ("where_root_column", &where_root_columns),
            ("where_parent_column", &where_parent_columns),
            ("where_completed_predicate", &where_completed_predicate),
            (
                "where_nested_relation_selector",
                &where_nested_relation_selector,
            ),
            ("where_rhs_column", &where_rhs_columns),
            (
                "relation_clause_exact_nested",
                &relation_clause_exact_nested,
            ),
            ("scalar_clause", &scalar_clause),
            ("unknown_clause", &unknown_clause),
            ("fragment_spread", &fragment_spread),
            ("title_body_after_clause", &title_body_after_clause),
            (
                "relation_path_clause_recovery",
                &relation_path_clause_recovery,
            ),
            (
                "relation_path_closed_clause_recovery",
                &relation_path_closed_clause_recovery,
            ),
            (
                "relation_path_after_fragment",
                &relation_path_after_fragment,
            ),
        ]
        .into_iter()
        .map(|(name, items)| format!("{name}\n{}", format_completions(items)))
        .collect::<Vec<_>>()
        .join("\n\n"),
    );
}

#[tokio::test]
async fn lsp_definitions_resolve_fragment_spreads_across_open_documents() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let fragment_uri = "file:///tests/queries/lsp/imdb-fragments.dsql".to_string();
    let query_uri = "file:///tests/queries/lsp/imdb-query.dsql".to_string();
    let fragment = "fragment MovieInfoFields on movie_info {\n  id\n  note\n}";
    let query = "query Movies {\n  movie_info {\n    ...MovieInfoFields\n  }\n}";
    host.open_document(fragment_uri.clone(), 1, fragment.to_string())
        .await;
    host.open_document(query_uri.clone(), 1, query.to_string())
        .await;

    let definition = host
        .definition(&query_uri, position_at(query, "MovieInfoFields"))
        .await
        .unwrap();

    assert_eq!(
        definition,
        DefinitionResult::Source(SourceDefinition {
            uri: fragment_uri,
            range: dsql_core::TextRange::new("fragment ".len(), "fragment MovieInfoFields".len()),
            kind: SourceDefinitionKind::Fragment,
        })
    );
}

#[tokio::test]
async fn lsp_definitions_resolve_tables_relations_and_columns_to_catalog_targets() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/queries/lsp/imdb-definitions.dsql".to_string();
    let source = "\
query Movies {
  movie_info(where .id == 1) {
    note
    title {
      id
    }
  }
}";
    host.open_document(uri.clone(), 1, source.to_string()).await;

    let table = host
        .definition(&uri, position_at(source, "movie_info"))
        .await
        .unwrap();
    let where_column = host
        .definition(&uri, position_at(source, "id == 1"))
        .await
        .unwrap();
    let body_column = host
        .definition(&uri, position_at(source, "note"))
        .await
        .unwrap();
    let relation = host
        .definition(&uri, position_at(source, "title {"))
        .await
        .unwrap();

    assert_eq!(
        table,
        DefinitionResult::Catalog(CatalogDefinition::Table {
            schema: "public".to_string(),
            table: "movie_info".to_string(),
        })
    );
    assert_eq!(
        where_column,
        DefinitionResult::Catalog(CatalogDefinition::Column {
            schema: "public".to_string(),
            table: "movie_info".to_string(),
            column: "id".to_string(),
        })
    );
    assert_eq!(
        body_column,
        DefinitionResult::Catalog(CatalogDefinition::Column {
            schema: "public".to_string(),
            table: "movie_info".to_string(),
            column: "note".to_string(),
        })
    );
    assert_eq!(
        relation,
        DefinitionResult::Catalog(CatalogDefinition::Table {
            schema: "public".to_string(),
            table: "title".to_string(),
        })
    );
}

fn format_completions(items: &[dsql_frontend::CompletionItem]) -> String {
    items
        .iter()
        .map(|item| {
            format!(
                "{:?} {} {}",
                completion_kind(item.kind),
                item.label,
                item.detail.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn completion_kind(kind: CompletionKind) -> CompletionKind {
    kind
}

fn position_at(source: &str, needle: &str) -> TextPosition {
    byte_to_position(source, source.find(needle).unwrap())
}

fn position_after(source: &str, needle: &str) -> TextPosition {
    byte_to_position(source, source.find(needle).unwrap() + needle.len())
}

async fn completions_at_marker(
    host: &AnalysisHost,
    uri: &str,
    source: &str,
    markers: &BTreeMap<String, usize>,
    marker: &str,
) -> Vec<dsql_frontend::CompletionItem> {
    let byte = *markers
        .get(marker)
        .unwrap_or_else(|| panic!("missing marker `{marker}`"));
    host.completions(uri, byte_to_position(source, byte))
        .await
        .unwrap()
}

fn assert_completion_labels(
    context: &str,
    items: &[dsql_frontend::CompletionItem],
    expected: &[&str],
) {
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    for expected_label in expected {
        assert!(
            labels.contains(expected_label),
            "{context} should include `{expected_label}`; got {labels:?}"
        );
    }
}

fn assert_no_completion_labels(
    context: &str,
    items: &[dsql_frontend::CompletionItem],
    unexpected: &[&str],
) {
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    for unexpected_label in unexpected {
        assert!(
            !labels.contains(unexpected_label),
            "{context} should not include `{unexpected_label}`; got {labels:?}"
        );
    }
}

fn marked_source(relative_path: &str) -> (String, BTreeMap<String, usize>) {
    let source = fs::read_to_string(fixture_root().join(relative_path)).unwrap();
    strip_markers(&source)
}

fn strip_markers(source: &str) -> (String, BTreeMap<String, usize>) {
    let mut output = String::new();
    let mut markers = BTreeMap::new();
    let mut rest = source;
    while let Some(start) = rest.find("/*^") {
        output.push_str(&rest[..start]);
        let marker_start = start + "/*^".len();
        let marker_end = rest[marker_start..].find("*/").unwrap() + marker_start;
        markers.insert(rest[marker_start..marker_end].to_string(), output.len());
        rest = &rest[marker_end + "*/".len()..];
    }
    output.push_str(rest);
    (output, markers)
}

fn byte_to_position(source: &str, byte: usize) -> TextPosition {
    let rope = ropey::Rope::from_str(source);
    let line = rope.byte_to_line_idx(byte, ropey::LineType::LF_CR);
    let line_start = rope.line_to_byte_idx(line, ropey::LineType::LF_CR);
    TextPosition {
        line: line as u32,
        character: rope.slice(line_start..byte).len_utf16() as u32,
    }
}

fn imdb_catalog() -> Catalog {
    dsql_project::load_metadata_dir(&fixture_root().join("schema/imdb"))
        .unwrap()
        .into_catalog()
        .unwrap()
        .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn snapshot(name: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(fixture_root().join("snapshots"));
    settings.bind(|| {
        insta::assert_snapshot!(name, contents);
    });
}
