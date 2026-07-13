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
                                text.to_text()
                                    .expect("editor text is resident")
                                    .replace("    id\n", "    idz\n")
                            } else {
                                text.to_text()
                                    .expect("editor text is resident")
                                    .replace("    idz\n", "    id\n")
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

/// Restoring a document's earlier content (A -> B -> A, a save-undo-save
/// flow) must leave exactly the facts a fresh load of A would produce.
/// The reproducing shape (bisected from the imdsql project): a
/// length-changing edit before a definition that selects a reverse
/// to-many relation with a clause — the restore answers FieldNotFound on
/// the clause's order-by column although the ResolvedClause facts are
/// correct post-settle, and the engine's explain reports the checker
/// memoized with stale views. Engine-level (porridge) staleness on
/// content revisit — see docs/issues.md; the daemon works around it by
/// full-reloading when a file revisits a previously-held content hash
/// (`plain_document_edits_roundtrip` in dsql-daemon pins that), and the
/// LSP path remains exposed. Un-ignore once the engine fix lands.
#[test]
#[ignore = "porridge staleness on content revisit; see docs/issues.md"]
fn content_roundtrip_edits_rederive_cleanly() {
    use bowl::Mut;
    use dsql_core::source::SourceText;

    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        let original = "fragment TitleBits on title {\n  id\n  title\n}\n\n\
                        fragment RatingBits on title {\n  ratings: movie_info_idx(where .info_type_id == 101 order by id asc limit 1) {\n    info\n  }\n}\n";
        let file = insert_source(&bowl, "roundtrip.dsql", original).await;
        assert_eq!(
            render_diagnostic_facts(&bowl).await,
            "",
            "the fixture starts clean"
        );

        let set_text = |content: String| {
            let bowl = &bowl;
            async move {
                let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
                for (entity, source) in sources.collect() {
                    if entity == file {
                        let content = content.clone();
                        source
                            .with_latest(move |text| text.set_text(&content))
                            .await;
                    }
                }
            }
        };

        let probed = original.replace(
            "fragment TitleBits on title {\n  id\n",
            "fragment TitleBits on title {\n  id\n  probe_year: production_year\n",
        );
        assert_ne!(probed, original, "the probe must apply");
        set_text(probed).await;
        assert_eq!(
            render_diagnostic_facts(&bowl).await,
            "",
            "the probe edit checks clean"
        );

        set_text(original.to_string()).await;
        assert_eq!(
            render_diagnostic_facts(&bowl).await,
            "",
            "restoring the original content checks clean"
        );
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

/// Well-formed uses of the registered directives check clean —
/// `@dsql.deprecated` on queries and fields (shorthand included). The one
/// exception is deliberate: `@dsql.include_if` validates fully but still
/// errors because its conditional-SQL semantics are not implemented;
/// accepting it would generate silently-unconditional SQL.
#[test]
fn registered_directives_check_against_their_schema() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        insert_source(
            &bowl,
            "annotated.dsql",
            "query Annotated @dsql.deprecated(reason: \"old\") {\n  title(limit 1) {\n    id @.deprecated\n    title @dsql.include_if(if: $flag)\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// Directive misuse reports the registry's diagnostics: unknown names,
/// illegal locations, and argument-schema violations, with the proof of
/// concept's precedence (a duplicate argument skips its own unknown/type
/// checks; a misplaced directive still checks arguments).
#[test]
fn directive_misuse_is_reported() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        insert_source(
            &bowl,
            "misuse.dsql",
            concat!(
                "fragment Bits on title {\n  episode_nr\n}\n",
                "query Misuse @dsql.include_if(if: 1) {\n",
                "  title(limit 1) @custom @foo.bar {\n",
                "    id @dsql.deprecated(reason: \"a\", reason: true, extra: 1)\n",
                "    title @dsql.include_if\n",
                "    kind_id @dsql.deprecated(reason: true)\n",
                "    production_year @.deprecated(bogus: 1)\n",
                "    ...Bits @dsql.deprecated\n",
                "  }\n",
                "}\n",
            ),
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// The boolean-expression argument accepts literals, variables, and
/// binary expressions; paths, strings, and numbers mismatch.
#[test]
fn directive_boolean_arguments_type_check() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        insert_source(
            &bowl,
            "booleans.dsql",
            concat!(
                "query Booleans {\n",
                "  title(limit 1) {\n",
                "    a: id @dsql.include_if(if: true)\n",
                "    b: id @dsql.include_if(if: $flag)\n",
                "    c: id @dsql.include_if(if: $a == $b)\n",
                "    d: id @dsql.include_if(if: .production_year)\n",
                "    e: id @dsql.include_if(if: \"yes\")\n",
                "    f: id @dsql.include_if(if: 1)\n",
                "    g: id @dsql.include_if(if: null)\n",
                "  }\n",
                "}\n",
            ),
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// Spread-introduced output keys are as ambiguous as local duplicates:
/// a local field colliding with a fragment field, and two fragments
/// providing the same key, both diagnose at the spread site.
#[test]
fn duplicate_output_keys_through_spreads_are_reported() {
    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        insert_source(
            &bowl,
            "fragments.dsql",
            "fragment IdBits on title {\n  id\n}\nfragment MoreIds on title {\n  id\n}\n",
        )
        .await;
        insert_source(
            &bowl,
            "spreads.dsql",
            "query LocalVsFragment {\n  title(limit 1) {\n    id\n    ...IdBits\n  }\n}\nquery FragmentVsFragment {\n  title(limit 1) {\n    ...IdBits\n    ...MoreIds\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// A same-length body edit moves no span, so nothing about the
/// definition's header changes — `DefDecl::source_hash` is what re-runs
/// the walks. Before it existed, this exact edit left diagnostics stale.
#[test]
fn same_length_body_edits_rederive_diagnostics() {
    use bowl::Mut;
    use dsql_core::source::SourceText;

    block_on(async {
        let bowl = checked_bowl(imdb_catalog()).await;
        let file = insert_source(
            &bowl,
            "same-length.dsql",
            "query Same {\n  title(limit 1) {\n    title\n  }\n}\n",
        )
        .await;
        assert_eq!(render_diagnostic_facts(&bowl).await, "");

        let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (entity, source) in sources.collect() {
            if entity == file {
                source
                    .with_latest(|text| {
                        // Same byte length: `title` -> `titlx`.
                        let edited = text
                            .to_text()
                            .expect("editor text is resident")
                            .replace("    title\n", "    titlx\n");
                        text.set_text(&edited);
                    })
                    .await;
            }
        }

        let diagnostics = render_diagnostic_facts(&bowl).await;
        assert!(
            diagnostics.contains("titlx"),
            "same-length body edits must re-run checks, got: {diagnostics:?}"
        );
    });
}
