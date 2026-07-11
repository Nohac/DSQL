//! Lints: unindexed-scan findings are advisory, severity-configurable, and
//! absent entirely without a lint configuration.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::facts::{DiagnosticsDemand, Severity};
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::insert_source;
use futures::executor::block_on;

use crate::{imdb_catalog, render_diagnostic_facts};

async fn linted_bowl(config: Option<LintConfig>) -> Bowl {
    let bowl = language_bowl().await;
    dsql_core::catalog::insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    if let Some(config) = config {
        bowl.insert((Singleton::<LintConfig>::new(), config)).await;
    }
    bowl
}

/// `aka_title->episode_of_id` joins over `episode_of_id`, which has no
/// index; `.aka_title->movie_id.title` scans `aka_title.title`, also
/// unindexed.
const SLOW_QUERY: &str = "query Slow {\n  title(where .aka_title->movie_id.title like \"%x%\") {\n    id\n    episodes: aka_title->episode_of_id {\n      id\n    }\n  }\n}\n";

#[test]
fn unindexed_joins_and_scans_are_flagged() {
    block_on(async {
        let bowl = linted_bowl(Some(LintConfig::default())).await;
        insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;
        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn lint_severity_follows_configuration() {
    block_on(async {
        let bowl = linted_bowl(Some(LintConfig {
            unindexed_scan_severity: Some(Severity::Warning),
        }))
        .await;
        insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;
        let rendered = render_diagnostic_facts(&bowl).await;
        assert!(rendered.contains("Warning["), "unexpected: {rendered}");
        assert!(!rendered.contains("Info["), "unexpected: {rendered}");
    });
}

#[test]
fn lints_are_off_without_configuration() {
    block_on(async {
        for config in [
            None,
            Some(LintConfig {
                unindexed_scan_severity: None,
            }),
        ] {
            let bowl = linted_bowl(config).await;
            insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;
            assert_eq!(render_diagnostic_facts(&bowl).await, "");
        }
    });
}

/// The demand marker gates lints like every diagnostic stage.
#[test]
fn lints_wait_for_diagnostics_demand() {
    block_on(async {
        let bowl = language_bowl().await;
        dsql_core::catalog::insert_catalog(&bowl, imdb_catalog()).await;
        bowl.insert((Singleton::<LintConfig>::new(), LintConfig::default()))
            .await;
        insert_source(&bowl, "slow.dsql", SLOW_QUERY).await;

        let rows = bowl
            .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
            .await;
        assert_eq!(rows.len(), 0, "no demand, no lints");
    });
}

/// Root-anchored predicate paths keep the pre-resolution behavior: only
/// current-anchored relation steps lint as nested scans. This pins the
/// deliberate choice recorded in `lint_predicates` — root paths need
/// their own rule before they warn.
#[test]
fn root_anchored_predicate_paths_do_not_lint() {
    block_on(async {
        let bowl = linted_bowl(Some(LintConfig::default())).await;
        insert_source(
            &bowl,
            "root-path.dsql",
            "query RootPath {\n  title(where ~aka_title->movie_id.title == \"x\" limit 1) {\n    id\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}
