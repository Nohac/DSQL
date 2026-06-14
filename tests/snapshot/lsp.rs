use dsql_core::{Catalog, SourceSnapshot, parse_source};
use dsql_frontend::{
    AnalysisHost, CatalogDefinition, CompletionKind, DefinitionResult, SourceDefinition,
    SourceDefinitionKind, TextPosition,
};
use insta::Settings;
use std::{fs, path::PathBuf};

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
async fn lsp_completion_context_ranges_cover_full_prefix_and_recovery_variants() {
    let fixture = context_fixture("queries/lsp/imdb-context-ranges.dsql");
    let variants = fixture.variants();
    assert!(
        !variants.is_empty(),
        "context fixture should produce variants"
    );

    for variant in variants {
        if variant.first_removed_byte.is_none() {
            assert_context_ranges(&variant, ContextMode::Full).await;
            assert_context_ranges(&variant, ContextMode::Prefix).await;
        } else {
            assert_context_ranges(&variant, ContextMode::Recovery).await;
        }
    }
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
async fn lsp_definitions_resolve_fragment_spreads_across_embedded_regions() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/src/movie-info.ts".to_string();
    let source = r#"import { dsql } from "@dsql/typescript";

export const TitleFields = dsql(`
fragment TitleFields on title {
  id
}
`);

export const Movies = dsql(`
query Movies {
  title {
    ...TitleFields
  }
}
`);
"#;
    host.open_document(uri.clone(), 1, source.to_string()).await;

    let definition = host
        .definition(&uri, position_after(source, "..."))
        .await
        .unwrap();

    let expected_start = source.find("fragment TitleFields").unwrap() + "fragment ".len();
    let expected_end = expected_start + "TitleFields".len();
    assert_eq!(
        definition,
        DefinitionResult::Source(SourceDefinition {
            uri,
            range: dsql_core::TextRange::new(expected_start, expected_end),
            kind: SourceDefinitionKind::Fragment,
        })
    );
}

#[tokio::test]
async fn lsp_completions_resolve_fragment_spreads_across_open_documents() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let fragment_uri = "file:///tests/queries/lsp/imdb-fragments.dsql".to_string();
    let query_uri = "file:///tests/queries/lsp/imdb-query.dsql".to_string();
    let fragment = "fragment TitleFields on title {\n  id\n}";
    let query = "query Movies {\n  title {\n    .";
    host.open_document(fragment_uri, 1, fragment.to_string())
        .await;
    host.open_document(query_uri.clone(), 1, query.to_string())
        .await;

    let completions = host
        .completions(&query_uri, position_after(query, "."))
        .await
        .unwrap();
    let fragment = completions
        .iter()
        .find(|item| item.label == "TitleFields")
        .expect("fragment completion from another open document should be present");

    assert_eq!(fragment.kind, CompletionKind::Fragment);
    assert_eq!(fragment.insert_text.as_deref(), Some("..TitleFields"));
}

#[tokio::test]
async fn lsp_completions_resolve_fragment_spreads_across_embedded_regions() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/src/movie-info.ts".to_string();
    let source = r#"import { dsql } from "@dsql/typescript";

export const TitleFields = dsql(`
fragment TitleFields on title {
  id
}
`);

export const Movies = dsql(`
query Movies {
  title {
    .
  }
}
`);
"#;
    host.open_document(uri.clone(), 1, source.to_string()).await;

    let completions = host
        .completions(&uri, position_after(source, "    ."))
        .await
        .unwrap();
    let fragment = completions
        .iter()
        .find(|item| item.label == "TitleFields")
        .expect("fragment completion from another embedded region should be present");

    assert_eq!(fragment.kind, CompletionKind::Fragment);
    assert_eq!(fragment.insert_text.as_deref(), Some("..TitleFields"));
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

#[tokio::test]
async fn lsp_hovers_show_inferred_variable_bindings() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/queries/lsp/imdb-variable-hover.dsql".to_string();
    let source = "\
query KeywordDiscovery {
  keyword(where .movie_keyword.title.production_year $year_op[>, >=] $ order by keyword $sort_dir limit $) {
    id
  }
}";
    host.open_document(uri.clone(), 1, source.to_string()).await;

    let where_variable = host
        .hover(&uri, position_at(source, "$ order"))
        .await
        .expect("where variable should have hover info");
    let limit_variable = host
        .hover(&uri, position_at(source, "$) {"))
        .await
        .expect("limit variable should have hover info");
    let operator_variable = host
        .hover(&uri, position_at(source, "$year_op"))
        .await
        .expect("operator variable should have hover info");
    let sort_variable = host
        .hover(&uri, position_at(source, "$sort_dir"))
        .await
        .expect("sort variable should have hover info");
    let query = host
        .hover(&uri, position_at(source, "KeywordDiscovery"))
        .await
        .expect("query should show inferred variable input shape");

    snapshot(
        "lsp_variable_hover",
        &format!(
            "{}\n\n{}\n\n{}\n\n{}\n\n{}",
            where_variable.markdown,
            limit_variable.markdown,
            operator_variable.markdown,
            sort_variable.markdown,
            query.markdown
        ),
    );
}

#[tokio::test]
async fn lsp_diagnostics_map_regex_embedded_dsql_ranges_to_host_document() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/src/movie-info.ts".to_string();
    let source = r#"import { dsql } from "@dsql/typescript";

export const MovieInfo = dsql(`
  query EmbeddedMovieInfoLookup {
    movie_info {
      missing_field
    }
  }
`);
"#;

    let diagnostics = host.open_document(uri.clone(), 1, source.to_string()).await;
    let field = diagnostics
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == dsql_core::DiagnosticCode::FieldNotFound)
        .expect("embedded invalid field should produce a mapped field diagnostic");

    let expected_start = source.find("missing_field").unwrap();
    let expected_end = expected_start + "missing_field".len();
    assert_eq!(
        field.range,
        dsql_core::TextRange::new(expected_start, expected_end)
    );
    assert_eq!(
        byte_to_position(source, field.range.start as usize),
        TextPosition {
            line: 5,
            character: 6
        }
    );
}

#[tokio::test]
async fn lsp_diagnostics_include_embedded_output_key_length_errors() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/src/movie-info.tsx".to_string();
    let long_alias =
        "this_alias_name_is_far_longer_than_postgresql_allows_for_identifiers_and_should_shrink";
    let source = format!(
        r#"import {{ dsql }} from "@dsql/typescript";

export const MovieInfo = dsql(`
  query EmbeddedMovieInfoLookup {{
    {long_alias}: movie_info {{
      id
    }}
  }}
`);
"#
    );

    let diagnostics = host.open_document(uri, 1, source.clone()).await;
    let diagnostic = diagnostics
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == dsql_core::DiagnosticCode::OutputKeyTooLong)
        .expect("embedded long output key should produce a mapped diagnostic");

    let expected_start = source.find(long_alias).unwrap();
    let expected_end = expected_start + long_alias.len();
    assert_eq!(
        diagnostic.range,
        dsql_core::TextRange::new(expected_start, expected_end)
    );
}

#[tokio::test]
async fn lsp_diagnostics_include_duplicate_fragments() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/queries/duplicates.dsql".to_string();
    let source = r#"fragment MovieFields on movie_info {
  id
}

fragment MovieFields on movie_info {
  info
}
"#;

    let diagnostics = host.open_document(uri, 1, source.to_string()).await;
    assert!(
        diagnostics.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == dsql_core::DiagnosticCode::DuplicateDefinition
                && diagnostic.message.contains("MovieFields")
        }),
        "duplicate fragment diagnostic missing: {:?}",
        diagnostics.diagnostics
    );
}

#[tokio::test]
async fn lsp_completions_only_activate_inside_regex_embedded_dsql() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/src/movie-info.ts".to_string();
    let source = r#"import { dsql } from "@dsql/typescript";

const outside =

export const MovieInfo = dsql(`
  query EmbeddedMovieInfoLookup {

  }
`);
"#;

    host.open_document(uri.clone(), 1, source.to_string()).await;

    let outside = host
        .completions(&uri, position_after(source, "const outside ="))
        .await
        .unwrap();
    assert!(
        outside.is_empty(),
        "host TypeScript outside an embedded DSQL region should not complete DSQL items; got {:?}",
        outside
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>()
    );

    let inside = host
        .completions(
            &uri,
            position_after(source, "  query EmbeddedMovieInfoLookup {\n"),
        )
        .await
        .unwrap();
    assert_completion_labels("embedded body", &inside, &["movie_info", "title"]);
}

#[tokio::test]
async fn lsp_formatting_rewrites_regex_embedded_dsql_region() {
    let host = AnalysisHost::new();
    host.set_catalog(imdb_catalog());
    let uri = "file:///tests/src/movie-info.ts".to_string();
    let source = r#"import { dsql } from "@dsql/typescript";

export const MovieInfo = dsql(`
query EmbeddedMovieInfoLookup { movie_info(limit 10) { id info title { id } } }
`);
"#;

    host.open_document(uri.clone(), 1, source.to_string()).await;

    let formatted = host
        .document_format(&uri)
        .await
        .expect("embedded document should format");

    assert_eq!(formatted.snapshot.uri, uri);
    assert_eq!(formatted.formatted.diagnostics, Vec::new());
    snapshot("lsp_embedded_formatting", &formatted.formatted.text);
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

fn context_fixture(relative_path: &str) -> ContextFixture {
    let source = fs::read_to_string(fixture_root().join(relative_path)).unwrap();
    ContextFixture {
        name: relative_path.to_string(),
        parts: parse_context_fixture_parts(&source),
    }
}

#[derive(Clone, Debug)]
struct ContextFixture {
    name: String,
    parts: Vec<FixturePart>,
}

impl ContextFixture {
    fn variants(&self) -> Vec<ContextVariant> {
        let mut variants = Vec::new();
        expand_context_fixture(
            &self.parts,
            0,
            ContextVariant {
                name: self.name.clone(),
                source: String::new(),
                markers: Vec::new(),
                first_removed_byte: None,
            },
            &mut variants,
        );
        variants
    }
}

#[derive(Clone, Debug)]
enum FixturePart {
    Text(String),
    Context(String),
    Optional {
        name: String,
        parts: Vec<FixturePart>,
    },
}

#[derive(Clone, Debug)]
struct ContextVariant {
    name: String,
    source: String,
    markers: Vec<ContextMarker>,
    first_removed_byte: Option<usize>,
}

#[derive(Clone, Debug)]
struct ContextMarker {
    id: Option<String>,
    expected: String,
    byte: usize,
}

#[derive(Clone, Copy, Debug)]
enum ContextMode {
    Full,
    Prefix,
    Recovery,
}

fn expand_context_fixture(
    parts: &[FixturePart],
    index: usize,
    current: ContextVariant,
    variants: &mut Vec<ContextVariant>,
) {
    let Some(part) = parts.get(index) else {
        variants.push(current);
        return;
    };

    match part {
        FixturePart::Text(text) => {
            let mut current = current;
            current.source.push_str(text);
            expand_context_fixture(parts, index + 1, current, variants);
        }
        FixturePart::Context(marker) => {
            let mut current = current;
            let (id, expected) = parse_context_marker(marker);
            current.markers.push(ContextMarker {
                id,
                expected,
                byte: current.source.len(),
            });
            expand_context_fixture(parts, index + 1, current, variants);
        }
        FixturePart::Optional {
            name,
            parts: optional_parts,
        } => {
            let mut skipped = current.clone();
            skipped.name = format!("{} {name}=off", skipped.name);
            skipped.first_removed_byte = Some(
                skipped
                    .first_removed_byte
                    .map_or(skipped.source.len(), |byte| byte.min(skipped.source.len())),
            );
            expand_context_fixture(parts, index + 1, skipped, variants);

            let mut included_variants = Vec::new();
            let mut included = current;
            included.name = format!("{} {name}=on", included.name);
            expand_context_fixture(optional_parts, 0, included, &mut included_variants);
            for included in included_variants {
                expand_context_fixture(parts, index + 1, included, variants);
            }
        }
    }
}

fn parse_context_marker(marker: &str) -> (Option<String>, String) {
    marker.find(':').map_or_else(
        || (None, marker.to_string()),
        |colon| {
            let id = marker[..colon].trim();
            let expected = marker[colon + 1..].trim();
            assert!(!id.is_empty(), "named context marker must have an id");
            assert!(
                !expected.is_empty(),
                "named context marker must have an expected context"
            );
            (Some(id.to_string()), expected.to_string())
        },
    )
}

fn parse_context_fixture_parts(source: &str) -> Vec<FixturePart> {
    let mut index = 0;
    let parts = parse_context_fixture_parts_until_close(source, &mut index, false);
    assert!(
        index == source.len(),
        "unexpected trailing fixture parser state at byte {index}"
    );
    parts
}

fn parse_context_fixture_parts_until_close(
    source: &str,
    index: &mut usize,
    stop_on_optional_close: bool,
) -> Vec<FixturePart> {
    let mut parts = Vec::new();
    while *index < source.len() {
        let Some(relative_start) = source[*index..].find("{%") else {
            parts.push(FixturePart::Text(source[*index..].to_string()));
            *index = source.len();
            break;
        };
        let tag_start = *index + relative_start;
        if tag_start > *index {
            parts.push(FixturePart::Text(source[*index..tag_start].to_string()));
        }
        if source[tag_start..].starts_with("{%?}") {
            *index = tag_start + "{%?}".len();
            assert!(
                stop_on_optional_close,
                "unexpected optional close tag at byte {tag_start}"
            );
            return parts;
        }
        let tag_body_start = tag_start + "{%".len();
        let tag_end = source[tag_body_start..]
            .find("%}")
            .map(|relative_end| tag_body_start + relative_end)
            .unwrap_or_else(|| panic!("unterminated fixture tag at byte {tag_start}"));
        let tag = source[tag_body_start..tag_end].trim();
        *index = tag_end + "%}".len();

        if let Some(optional_tag) = tag.strip_prefix('?') {
            let optional_name = optional_tag.trim();
            if optional_name.is_empty() {
                assert!(
                    stop_on_optional_close,
                    "unexpected optional close tag at byte {tag_start}"
                );
                return parts;
            }
            let optional_parts = parse_context_fixture_parts_until_close(source, index, true);
            parts.push(FixturePart::Optional {
                name: optional_name.to_string(),
                parts: optional_parts,
            });
        } else {
            assert!(!tag.is_empty(), "empty context tag at byte {tag_start}");
            parts.push(FixturePart::Context(tag.to_string()));
        }
    }

    assert!(
        !stop_on_optional_close,
        "unterminated optional fixture block before end of file"
    );
    parts
}

async fn assert_context_ranges(variant: &ContextVariant, mode: ContextMode) {
    let ranges = context_ranges(variant);
    let trace = std::env::var_os("DSQL_TRACE_LSP_CONTEXTS").is_some();
    for (range_index, range) in ranges.iter().enumerate() {
        for byte in range.start..range.end {
            if !should_assert_context_at(&variant.source, byte) {
                continue;
            }
            let (source, position) = match mode {
                ContextMode::Full => (
                    variant.source.clone(),
                    byte_to_position(&variant.source, byte),
                ),
                ContextMode::Prefix => {
                    let source = variant.source[..byte].to_string();
                    let position = byte_to_position(&source, source.len());
                    (source, position)
                }
                ContextMode::Recovery => {
                    if byte >= variant.first_removed_byte.unwrap_or(variant.source.len()) {
                        continue;
                    }
                    let source = variant.source[..byte].to_string();
                    let position = byte_to_position(&source, source.len());
                    (source, position)
                }
            };
            let uri = format!(
                "file:///tests/queries/lsp/context-ranges-{}-{range_index}.dsql",
                match mode {
                    ContextMode::Full => "full",
                    ContextMode::Prefix => "prefix",
                    ContextMode::Recovery => "recovery",
                }
            );
            let host = AnalysisHost::new();
            host.set_catalog(imdb_catalog());
            host.open_document(uri.clone(), 1, source.clone()).await;
            let actual = host
                .completion_context_debug(&uri, position)
                .await
                .expect("context fixture document should be open");
            if trace {
                trace_context_step(variant, mode, range, byte, &actual);
            }
            assert_eq!(
                actual,
                range.expected,
                "unexpected completion context in {} mode for {} at byte {byte}\nsource prefix:\n{}",
                match mode {
                    ContextMode::Full => "full",
                    ContextMode::Prefix => "prefix",
                    ContextMode::Recovery => "recovery",
                },
                variant.name,
                &variant.source[..byte]
            );
            if let Some(id) = &range.id {
                let items = host
                    .completions(&uri, position)
                    .await
                    .expect("context fixture document should be open");
                if trace {
                    trace_context_completions(id, &items);
                }
                assert_named_context_completions(id, range, &variant.source, byte, &items);
            }
        }
    }
}

fn trace_context_step(
    variant: &ContextVariant,
    mode: ContextMode,
    range: &ContextRange,
    byte: usize,
    actual: &str,
) {
    let next = variant.source[byte..].chars().next().unwrap_or('\0');
    eprintln!(
        "{mode:?} {} byte={byte} expected={} actual={} next={next:?} id={}",
        variant.name,
        range.expected,
        actual,
        range.id.as_deref().unwrap_or("-")
    );
}

fn trace_context_completions(id: &str, items: &[dsql_frontend::CompletionItem]) {
    let completions = items
        .iter()
        .map(|item| {
            format!(
                "{} -> {}",
                item.label,
                item.insert_text.as_deref().unwrap_or("<default>")
            )
        })
        .collect::<Vec<_>>();
    eprintln!("  {id} completions: {completions:?}");
}

struct ContextRange {
    id: Option<String>,
    expected: String,
    start: usize,
    end: usize,
}

fn context_ranges(variant: &ContextVariant) -> Vec<ContextRange> {
    variant
        .markers
        .iter()
        .enumerate()
        .filter_map(|(index, marker)| {
            let end = variant
                .markers
                .get(index + 1)
                .map_or(variant.source.len(), |next| next.byte);
            (marker.byte < end).then(|| ContextRange {
                id: marker.id.clone(),
                expected: marker.expected.clone(),
                start: marker.byte,
                end,
            })
        })
        .collect()
}

fn assert_named_context_completions(
    id: &str,
    range: &ContextRange,
    source: &str,
    byte: usize,
    items: &[dsql_frontend::CompletionItem],
) {
    match id {
        "TitleFieldsFragment" => {
            let completion = items
                .iter()
                .find(|item| item.label == "TitleFields")
                .unwrap_or_else(|| {
                    panic!(
                        "{id} should include TitleFields completion at byte {byte}; got {:?}",
                        items
                            .iter()
                            .map(|item| item.label.as_str())
                            .collect::<Vec<_>>()
                    )
                });
            let typed = &source[range.start - 1..byte];
            let expected_insert = "...TitleFields"
                .strip_prefix(typed)
                .unwrap_or("...TitleFields");
            assert_eq!(
                completion.insert_text.as_deref(),
                Some(expected_insert),
                "{id} should complete partial fragment spread `{typed}` at byte {byte}"
            );
        }
        other => panic!("unknown named context marker `{other}`"),
    }
}

fn should_assert_context_at(source: &str, byte: usize) -> bool {
    let previous = source[..byte].chars().next_back();
    source[byte..].chars().next().is_some_and(|character| {
        !character.is_whitespace()
            && character != '"'
            && !inside_string_literal(source, byte)
            && !(is_identifier_char(character) && previous.is_some_and(is_identifier_char))
            && !(is_operator_char(character) && previous.is_some_and(is_operator_char))
    })
}

fn is_identifier_char(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

fn is_operator_char(character: char) -> bool {
    matches!(character, '=' | '!' | '<' | '>')
}

fn inside_string_literal(source: &str, byte: usize) -> bool {
    source[..byte]
        .chars()
        .rev()
        .take_while(|character| *character != '\n')
        .filter(|character| *character == '"')
        .count()
        % 2
        == 1
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
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!(format!("lsp__{name}"), contents);
    });
}
