//! Formatting: canonical layout for well-formed sources, comment and user
//! line-break preservation, and original text for files with parse errors.

use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;

fn format(source: &str) -> dsql_core::format::FormattedText {
    let (cst, diagnostics) = parse(source);
    format_document(&cst.into_data(), source, !diagnostics.is_empty())
}

/// The reference formatter cases, verbatim sources.
#[test]
fn formatter_cases_match_expected_output() {
    let cases = [
        (
            "selection_boundaries",
            "query Users { users(where .id > 18 order by name desc limit 10) { id name } }",
        ),
        (
            "comma_line_groups",
            "query Users { users { id, name, email } }",
        ),
        (
            "complex_where_clauses",
            "query CastLookupForHauntedMovieInfo { movie_info(where (.info like \"%haunted%\" and .id > 10 and .movie_id == 10) or .id == 10 order by id asc limit 10) { id } }",
        ),
        (
            "short_clause_linebreaks",
            "query Movies { movie_info(limit 10\norder by id desc) { id } }",
        ),
        (
            "clause_line_groups",
            "query Movies { title(where .aka_title->episode_of_id.episode_nr > 0\norder by production_year desc limit 25) { id } }",
        ),
        (
            "variables_in_clauses",
            "query KeywordDiscovery { keyword(where .movie_keyword.title.production_year $[>, >=] $ order by keyword $sort_dir limit $movie_limit offset $$) { id } }",
        ),
        (
            "long_inline_clauses",
            "query Movies { title(where .aka_title->episode_of_id.episode_nr > 0 order by production_year desc limit 25 offset 12345) { id } }",
        ),
        ("comment_trivia", "query Users { # ids\n id }"),
    ];

    let output = cases
        .iter()
        .map(|(name, source)| {
            let formatted = format(source);
            assert_eq!(
                formatted.confidence,
                FormatConfidence::Full,
                "{name} must format fully"
            );
            format!("{name}\n{}", formatted.text.trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    insta::assert_snapshot!(output);
}

#[test]
fn formatting_is_idempotent() {
    let source =
        "query Users { users(where .id > 18 and (.name like \"a%\" or .email like \"b%\") order by name desc limit 10) { id posts { title } ...Extra } }\nfragment Extra on users {\n  email\n}\n";
    let once = format(source);
    let twice = format(&once.text);
    assert_eq!(once.text, twice.text, "formatting must be idempotent");
}

#[test]
fn parse_errors_preserve_the_original_text() {
    let source = "query Broken {\n  title(where\n}\n";
    let formatted = format(source);
    assert_eq!(formatted.confidence, FormatConfidence::PreserveOriginal);
    assert_eq!(formatted.text, source);
}
