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
fn aggregate_transform_is_contextual_syntax() {
    let source = concat!(
        "query Summary {\n",
        "  stats: title(where .production_year > 2000) | aggregate {\n",
        "    count\n",
        "    earliest: min .production_year\n",
        "  }\n",
        "}\n",
    );
    let (cst, diagnostics) = parse(source);
    let rendered = render_diagnostics(source, &diagnostics);
    insta::assert_snapshot!(format!("{cst}---\n{rendered}"));
}

#[test]
fn scalar_aggregate_predicates_bind_before_comparisons_and_clauses() {
    let source = concat!(
        "query AggregateFilters {\n",
        "  title(\n",
        "    where .movie_info_idx | exists\n",
        "      and .movie_info_idx | count >= $$minimum\n",
        "      and (.movie_info_idx | min .info) like \"4.%\"\n",
        "    limit 10\n",
        "  ) { id }\n",
        "}\n",
    );
    let (cst, diagnostics) = parse(source);
    let rendered = render_diagnostics(source, &diagnostics);
    insta::assert_snapshot!(format!("{cst}---\n{rendered}"));
}

#[test]
fn spreads_and_flattened_selections_are_disambiguated_by_the_suffix() {
    let source = concat!(
        "fragment Bits on users { id }\n",
        "query Flattened {\n",
        "  accounts: users(limit 1) {\n",
        "    ...Bits @include(if: true)\n",
        "    ...posts @include(if: true) { title }\n",
        "    ...posts | aggregate { post_count: count }\n",
        "  }\n",
        "  ...public::users(where .name == $$name) | aggregate { user_count: count }\n",
        "}\n",
    );
    let (cst, diagnostics) = parse(source);
    let rendered = render_diagnostics(source, &diagnostics);
    insta::assert_snapshot!(format!("{cst}---\n{rendered}"));
}

#[test]
fn directives_are_rejected_at_aggregate_owned_positions() {
    let sources = [
        "query Q { title @audit | aggregate { count } }",
        "query Q { title | aggregate @audit { count } }",
        "query Q { title | aggregate { @audit count } }",
    ];
    let rendered = sources
        .iter()
        .map(|source| {
            let (_, diagnostics) = parse(source);
            render_diagnostics(source, &diagnostics)
        })
        .collect::<Vec<_>>()
        .join("---\n");
    insta::assert_snapshot!(rendered);
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
