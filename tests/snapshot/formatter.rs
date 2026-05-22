use dsql_core::{SourceSnapshot, format_file, parse_source};
use insta::Settings;
use std::path::PathBuf;

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
            "query Movies { title(where .aka_title::episode_of_id.episode_nr > 0\norder by production_year desc limit 25) { id } }",
        ),
        (
            "variables_in_clauses",
            "query KeywordDiscovery { keyword(where .movie_keyword.title.production_year $[>, >=] $ order by keyword $sort_dir limit $movie_limit offset $$) { id } }",
        ),
        (
            "long_inline_clauses",
            "query Movies { title(where .aka_title::episode_of_id.episode_nr > 0 order by production_year desc limit 25 offset 12345) { id } }",
        ),
        ("comment_trivia", "query Users { # ids\n id }"),
    ];

    let output = cases
        .iter()
        .map(|(name, source)| {
            let parsed = parse_source(SourceSnapshot::from(*source));
            let formatted = format_file(&parsed);
            assert!(
                formatted.diagnostics.is_empty(),
                "{name} diagnostics: {:?}",
                formatted.diagnostics
            );
            format!("{name}\n{}", formatted.text.trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    snapshot("formatter_cases", &output);
}

fn snapshot(name: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(fixture_root().join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!(format!("formatter__{name}"), contents);
    });
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
