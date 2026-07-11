//! Catalog checks: valid fixtures produce no diagnostics against the imdb
//! schema; invalid fixtures and inline sources produce check diagnostics,
//! demand-gated and catalog-reactive.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{Diagnostic, DiagnosticsDemand};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;
use futures::executor::block_on;

use crate::{fixture, imdb_catalog, render_diagnostic_facts};

async fn checked_bowl(catalog: Catalog) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl
}

#[test]
fn valid_fixtures_check_clean_against_imdb() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;

        for name in [
            "valid/imdb-title-basic.dsql",
            "valid/imdb-movie-info-basic.dsql",
            "valid/imdb-relation-path-selector.dsql",
            "valid/imdb-scoped-relation-predicate.dsql",
            "valid/imdb-fragment-spread.dsql",
        ] {
            insert_source(&bowl, name, &fixture(name)).await;
        }

        let diagnostics = render_diagnostic_facts(&bowl).await;
        assert_eq!(diagnostics, "", "valid fixtures must check clean");
    });
}

#[test]
fn invalid_fixtures_report_diagnostics_against_imdb() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;

        insert_source(
            &bowl,
            "invalid/imdb-unknown-column.dsql",
            &fixture("invalid/imdb-unknown-column.dsql"),
        )
        .await;
        insert_source(
            &bowl,
            "invalid/imdb-scalar-clause-list.dsql",
            &fixture("invalid/imdb-scalar-clause-list.dsql"),
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn selection_checks_report_ported_diagnostics() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;

        insert_source(
            &bowl,
            "mixed.dsql",
            concat!(
                "query Mixed {\n",
                "  missing_table {\n    id\n  }\n",
                "  title(limit 1) {\n",
                "    id\n",
                "    id\n",
                "    title {\n      id\n    }\n",
                "    kind_type\n",
                "  }\n",
                "}\n",
            ),
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn fragment_checks_report_target_and_compat_mismatches() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;

        insert_source(
            &bowl,
            "fragments.dsql",
            concat!(
                "fragment OnMissing on nope {\n  id\n}\n",
                "fragment KindFields on kind_type {\n  kind\n}\n",
                "fragment Loop on title {\n  id\n  ...Loop\n}\n",
                "query Q {\n  title(limit 1) {\n    ...KindFields\n  }\n}\n",
            ),
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn catalog_replacement_retires_and_recomputes_diagnostics() {
    block_on(async {
        let bowl = checked_bowl(Catalog::hardcoded()).await;

        insert_source(
            &bowl,
            "users.dsql",
            "query Users {\n  users {\n    id\n    nickname\n  }\n}\n",
        )
        .await;

        let before = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(before, 1, "unknown column must be reported");

        // Replace the catalog with one that knows the column is gone too —
        // the imdb catalog has no `users` table at all, so the diagnostic
        // must change shape rather than survive stale.
        insert_catalog(&bowl, imdb_catalog()).await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// A field rename is a member-side *value* change: the entity ids — and
/// with them the engine-maintained `Children`/`FieldResolutions` inverses
/// the resolver's `In` joins pair through — are unchanged, so the provider
/// row never moves and only the pair's member does. This is the exact
/// case delta-planned bound joins can go stale on; diagnostics must
/// follow every toggle.
#[test]
fn member_value_edits_rederive_resolution() {
    use bowl::Mut;
    use dsql_core::source::SourceText;

    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        let file = insert_source(
            &bowl,
            "edit.dsql",
            "query Edit {\n  title {\n    id\n  }\n}\n",
        )
        .await;
        assert_eq!(
            render_diagnostic_facts(&bowl).await,
            "",
            "the fixture starts clean"
        );

        for round in 0..4 {
            let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
            for (entity, source) in sources.collect() {
                if entity == file {
                    source
                        .with_latest(move |text| {
                            let edited = if round % 2 == 0 {
                                text.to_text().replace("    id\n", "    idz\n")
                            } else {
                                text.to_text().replace("    idz\n", "    id\n")
                            };
                            text.set_text(&edited);
                        })
                        .await;
                }
            }
            let diagnostics = render_diagnostic_facts(&bowl).await;
            if round % 2 == 0 {
                assert!(
                    diagnostics.contains("idz"),
                    "round {round}: the renamed column must re-check, got: {diagnostics:?}"
                );
            } else {
                assert_eq!(
                    diagnostics, "",
                    "round {round}: renaming back must retire the diagnostic"
                );
            }
        }
    });
}

/// Two operations with one name in one scope collide at the artifact
/// boundary, so they are language errors like duplicate fragments.
#[test]
fn duplicate_operation_names_are_reported() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        insert_source(
            &bowl,
            "one.dsql",
            "query Movies {\n  title(limit 1) {\n    id\n  }\n}\n",
        )
        .await;
        insert_source(
            &bowl,
            "two.dsql",
            "query Movies {\n  title(limit 2) {\n    id\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// Directives parse and lower, but no directive semantics are ported yet:
/// accepting them as silent no-ops would drop the behavior the directive
/// spec promises, so every use is an error until the registry lands.
#[test]
fn directives_are_rejected_as_unsupported() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        insert_source(
            &bowl,
            "annotated.dsql",
            "query Annotated @dsql.include_if(condition: $flag) {\n  title(limit 1) {\n    id @.deprecated(reason: \"old\")\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}
