//! CST snapshots: every fixture query parses to a lossless tree, and parse
//! errors surface as diagnostics with structured expected-token records.

use dsql_core::grammar::parse;

use crate::{fixture, render_diagnostics};

fn cst_snapshot(relative_path: &str) -> String {
    let source = fixture(relative_path);
    let (cst, diagnostics) = parse(&source);
    let mut snapshot = format!("{cst}");
    let rendered = render_diagnostics(&source, &diagnostics);
    if !rendered.is_empty() {
        snapshot.push_str("---\n");
        snapshot.push_str(&rendered);
    }
    snapshot
}

#[test]
fn valid_title_basic() {
    insta::assert_snapshot!(cst_snapshot("valid/imdb-title-basic.dsql"));
}

#[test]
fn valid_movie_info_basic() {
    insta::assert_snapshot!(cst_snapshot("valid/imdb-movie-info-basic.dsql"));
}

#[test]
fn valid_relation_path_selector() {
    insta::assert_snapshot!(cst_snapshot("valid/imdb-relation-path-selector.dsql"));
}

#[test]
fn valid_scoped_relation_predicate() {
    insta::assert_snapshot!(cst_snapshot("valid/imdb-scoped-relation-predicate.dsql"));
}

#[test]
fn invalid_scalar_clause_list() {
    insta::assert_snapshot!(cst_snapshot("invalid/imdb-scalar-clause-list.dsql"));
}

#[test]
fn syntax_error_reports_expected_tokens() {
    let source = "query Broken {\n  title(where .id ==\n}\n";
    let (cst, diagnostics) = parse(source);
    let mut expected_tokens: Vec<String> = cst
        .expected_tokens()
        .iter()
        .map(|expected| format!("{:?}@{:?}", expected.token, expected.span))
        .collect();
    expected_tokens.sort();
    insta::assert_snapshot!(format!(
        "diagnostics:\n{}expected tokens:\n{}",
        render_diagnostics(source, &diagnostics),
        expected_tokens.join("\n"),
    ));
}
