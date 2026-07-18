//! Catalog checks: valid fixtures produce no diagnostics against the imdb
//! schema; invalid fixtures and inline sources produce check diagnostics,
//! demand-gated and catalog-reactive.

use bowl::{Bowl, Entity, Query};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{Diagnostic, arm_editor_demands};
use dsql_core::language_bowl;
use dsql_core::source::{insert_embedding_source, insert_source};

use crate::{
    fixture, imdb_catalog, numeric_catalog, render_diagnostic_facts, replace_source_text,
    set_source_text,
};

async fn checked_bowl(catalog: Catalog) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    arm_editor_demands(&bowl).await;
    bowl
}

#[tokio::test]
async fn duplicate_anonymous_variables_are_reported_for_queries_and_fragments() {
    let bowl = checked_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "anonymous.dsql",
        concat!(
            "query AmbiguousQuery {\n",
            "  title(where .id > $ and .id < $ limit 1) {\n    id\n  }\n",
            "}\n",
            "fragment AmbiguousFragment on title {\n",
            "  movie_info(where .id > $ and .id < $) {\n    id\n  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn valid_fixtures_check_clean_against_imdb() {
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
}

#[tokio::test]
async fn numeric_and_float_literals_follow_their_logical_types() {
    let bowl = checked_bowl(numeric_catalog()).await;
    insert_source(
        &bowl,
        "numeric-literals.dsql",
        concat!(
            "query NumericLiterals {\n",
            "  metrics(where .amount >= 12345678901234567890.12345678901234567890",
            " and .ratio == \"not a float\") {\n",
            "    amount\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn predicate_extensions_report_invalid_shapes_and_types() {
    let bowl = checked_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "invalid-predicates.dsql",
        concat!(
            "query InvalidPredicates {\n",
            "  title(where .id in [\"wrong\"]\n",
            "    or 1 in [1]\n",
            "    or exists .id\n",
            "    or exists .missing\n",
            "    or $$value is null\n",
            "    or [1]\n",
            "    or not [2]\n",
            "    or .production_year\n",
            "    or .movie_info_idx.info_type_id\n",
            "    or \"not boolean\") { id }\n",
            "}\n",
        ),
    )
    .await;

    let numeric_bowl = checked_bowl(numeric_catalog()).await;
    insert_source(
        &numeric_bowl,
        "bare-boolean-field.dsql",
        "query BareBooleanField { metrics(where .enabled) { amount } }",
    )
    .await;

    insta::assert_snapshot!(format!(
        "mixed predicates:\n{}\n\nboolean field:\n{}",
        render_diagnostic_facts(&bowl).await,
        render_diagnostic_facts(&numeric_bowl).await,
    ));
}

#[tokio::test]
async fn invalid_fixtures_report_diagnostics_against_imdb() {
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
    insert_source(
        &bowl,
        "invalid/imdb-duplicate-relation-path.dsql",
        &fixture("invalid/imdb-duplicate-relation-path.dsql"),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn table_resolution_spans_all_visible_schemas() {
    let catalog = Catalog::hardcoded().with_default_schema("other_schema");
    let bowl = checked_bowl(catalog).await;
    insert_source(
        &bowl,
        "cross-schema.dsql",
        concat!(
            "fragment AmbiguousTarget on users { id }\n",
            "query CrossSchema {\n",
            "  recent: posts(limit 1) { id }\n",
            "  public_users: public::users(limit 1) { id }\n",
            "  other_users: other_schema::users(limit 1) { id }\n",
            "  users(limit 1) { id }\n",
            "  missing { id }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn selection_checks_report_ported_diagnostics() {
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
}

#[tokio::test]
async fn valid_singular_and_aggregate_flattening_check_cleanly() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "flattened-valid.dsql",
        concat!(
            "fragment FlatOwner on posts {\n",
            "  ...users(where .name like $$owner) { owner_name: name }\n",
            "}\n",
            "query Flattened {\n",
            "  feed: posts(limit 2) { id ...FlatOwner }\n",
            "  accounts: public::users(limit 1) {\n",
            "    id\n",
            "    ...posts(where .title like $$title) | aggregate { post_count: count }\n",
            "  }\n",
            "  ...public::users(where .name == $$root_name) | aggregate { user_count: count }\n",
            "  ...public::users(limit 1) { flattened_user_id: id }\n",
            "  one_account: public::users(limit 1) {\n",
            "    ...posts(limit 1) { latest_post_title: title }\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    let diagnostics = render_diagnostic_facts(&bowl).await;
    assert_eq!(diagnostics, "", "valid flattening must check clean");
}

#[tokio::test]
async fn selection_shape_warnings_and_null_operator_errors_are_typed() {
    let bowl = checked_bowl(imdb_catalog()).await;
    insert_source(
        &bowl,
        "shape-diagnostics.dsql",
        concat!(
            "query ShapeDiagnostics {\n",
            "  empty: title(limit 0) { id }\n",
            "  redundant_literal: title(where .id == $$id limit 1) { id }\n",
            "  redundant_runtime: title(where .id == $$other_id limit $$cap) { id }\n",
            "  valid_limit_proof: title(limit 1) { id }\n",
            "  invalid_null_order: title(where .id > null) { id }\n",
            "  invalid_null_variant: title(where .title $$operator[==, like] null) { id }\n",
            "  parent: title(limit 1) {\n",
            "    kind_type(limit 1) { id }\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn invalid_flattening_cardinality_bodies_and_collisions_are_reported() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;
    insert_source(
        &bowl,
        "flattened-invalid.dsql",
        concat!(
            "fragment FlatOwner on posts {\n",
            "  ...users { owner_name: name }\n",
            "}\n",
            "query InvalidFlattening {\n",
            "  ...public::users { id }\n",
            "  accounts: public::users(limit 1) {\n",
            "    ...posts { title }\n",
            "    ...name { id }\n",
            "    ...email()\n",
            "    post_count: id\n",
            "    ...posts | aggregate { post_count: count }\n",
            "  }\n",
            "  feed: posts(limit 1) {\n",
            "    owner_name: title\n",
            "    ...FlatOwner\n",
            "  }\n",
            "}\n",
        ),
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn fragment_checks_report_target_and_compat_mismatches() {
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
}

#[tokio::test]
async fn catalog_replacement_retires_and_recomputes_diagnostics() {
    let bowl = checked_bowl(Catalog::hardcoded()).await;

    insert_source(
        &bowl,
        "users.dsql",
        "query Users {\n  public::users {\n    id\n    nickname\n  }\n}\n",
    )
    .await;

    let before = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
    assert_eq!(before, 1, "unknown column must be reported");

    // Replace the catalog with one that knows the column is gone too —
    // the imdb catalog has no `users` table at all, so the diagnostic
    // must change shape rather than survive stale.
    insert_catalog(&bowl, imdb_catalog()).await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

/// A field rename is a member-side *value* change: the entity ids — and
/// with them the engine-maintained `Children`/`FieldResolutions` inverses
/// the resolver's `In` joins pair through — are unchanged, so the provider
/// row never moves and only the pair's member does. This is the exact
/// case delta-planned bound joins can go stale on; diagnostics must
/// follow every toggle.
#[tokio::test]
async fn member_value_edits_rederive_resolution() {
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
        let (from, to) = if round % 2 == 0 {
            ("    id\n", "    idz\n")
        } else {
            ("    idz\n", "    id\n")
        };
        replace_source_text(&bowl, file, from, to).await;
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
}

/// Restoring a document's earlier content (A -> B -> A, a save-undo-save
/// flow) must leave exactly the facts a fresh load of A would produce.
/// The reproducing shape (bisected from the imdsql project): a
/// length-changing edit before a definition that selects a reverse
/// to-many relation with a clause. Pinned against the engine's ambient
/// healing (porridge bdebf49) plus its settle-phase extension — reaps
/// moving viewed stores after the last healing pass (porridge 8670456).
/// The current pin also carries the derived-pair reconciliation in
/// porridge e81194e; the history is in docs/issues.md.
#[tokio::test]
async fn content_roundtrip_edits_rederive_cleanly() {
    let bowl = checked_bowl(imdb_catalog()).await;
    let original = "fragment TitleBits on title {\n  id\n  title\n}\n\n\
                        fragment RatingBits on title {\n  ratings: movie_info_idx(where .info_type_id == 101 order by id asc limit 1) {\n    info\n  }\n}\n";
    let file = insert_source(&bowl, "roundtrip.dsql", original).await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "the fixture starts clean"
    );

    let probed = original.replace(
        "fragment TitleBits on title {\n  id\n",
        "fragment TitleBits on title {\n  id\n  probe_year: production_year\n",
    );
    assert_ne!(probed, original, "the probe must apply");
    set_source_text(&bowl, file, probed).await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "the probe edit checks clean"
    );

    set_source_text(&bowl, file, original).await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "restoring the original content checks clean"
    );
}

/// The host-file variant of the roundtrip: the revisited content lives
/// in an extracted REGION (a derived entity), not a base document. This
/// shape (bisected from imdsql: a fragment chain from another file plus
/// sibling selections, one repeating a fragment-selected relation under
/// another alias) exposed a layer deeper than ambient-view healing:
/// post-restore the current `full_cast` field had no ResolvedSelection
/// while a ghost resolution anchored on a removed intermediate-era
/// field entity survived cleanup. Porridge e81194e reconciles removed
/// pair-bound invocations to a fixed point; see docs/issues.md.
#[tokio::test]
async fn content_roundtrip_edits_rederive_cleanly_for_hosts() {
    let bowl = checked_bowl(imdb_catalog()).await;
    // The daemon arms the full generate pipeline: plan, variables,
    // and SQL stages all consume resolutions ambiently at Complete.
    dsql_core::facts::arm_generate_demands(&bowl).await;
    // The clause-bearing selections live in a FRAGMENT CHAIN in
    // another file, and the host adds sibling selections of its own —
    // one repeating a fragment-selected relation under another alias
    // (bisected from the imdsql project; smaller shapes heal fine).
    insert_source(
            &bowl,
            "fragments.dsql",
            "fragment CompactBits on title {\n  id\n}\n\nfragment HeroBits on title {\n  ...CompactBits\n  cast: cast_info(order by nr_order asc limit 5) {\n    nr_order\n  }\n}\n",
        )
        .await;
    let original = "export const Q = dsql(`\nquery Roundtrip {\n  title(where .id == $$movieId) {\n    ...HeroBits\n    keywords: movie_keyword(order by id asc limit 14) {\n      keyword {\n        keyword\n      }\n    }\n    full_cast: cast_info(order by nr_order asc limit 14) {\n      id\n      nr_order\n    }\n  }\n}\n`);\n";
    let file = insert_embedding_source(&bowl, "roundtrip.host", original, "typescript").await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "the fixture starts clean"
    );

    let probed = original.replace(
        "    ...HeroBits\n",
        "    probe_year: production_year\n    ...HeroBits\n",
    );
    assert_ne!(probed, original, "the probe must apply");
    set_source_text(&bowl, file, probed).await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "the probe edit checks clean"
    );

    set_source_text(&bowl, file, original).await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "restoring the original content checks clean"
    );
}

/// Two operations with one name in one scope collide at the artifact
/// boundary, so they are language errors like duplicate fragments.
#[tokio::test]
async fn duplicate_operation_names_are_reported() {
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
}

/// Well-formed uses of the registered directives check clean —
/// `@dsql.deprecated` on queries and fields (shorthand included). The one
/// exception is deliberate: `@dsql.include_if` validates fully but still
/// errors because its conditional-SQL semantics are not implemented;
/// accepting it would generate silently-unconditional SQL.
#[tokio::test]
async fn registered_directives_check_against_their_schema() {
    let bowl = checked_bowl(imdb_catalog()).await;
    insert_source(
            &bowl,
            "annotated.dsql",
            "query Annotated @dsql.deprecated(reason: \"old\") {\n  title(limit 1) {\n    id @.deprecated\n    title @dsql.include_if(if: $flag)\n  }\n}\n",
        )
        .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

/// Directive misuse reports the registry's diagnostics: unknown names,
/// illegal locations, and argument-schema violations, with the proof of
/// concept's precedence (a duplicate argument skips its own unknown/type
/// checks; a misplaced directive still checks arguments).
#[tokio::test]
async fn directive_misuse_is_reported() {
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
}

/// The boolean-expression argument accepts literals, variables, and
/// binary expressions; paths, strings, and numbers mismatch.
#[tokio::test]
async fn directive_boolean_arguments_type_check() {
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
}

/// Spread-introduced output keys are as ambiguous as local duplicates:
/// a local field colliding with a fragment field, and two fragments
/// providing the same key, both diagnose at the spread site.
#[tokio::test]
async fn duplicate_output_keys_through_spreads_are_reported() {
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
}

/// A same-length body edit moves no span, so nothing about the
/// definition's header changes — `DefDecl::source_hash` is what re-runs
/// the walks. Before it existed, this exact edit left diagnostics stale.
#[tokio::test]
async fn same_length_body_edits_rederive_diagnostics() {
    let bowl = checked_bowl(imdb_catalog()).await;
    let file = insert_source(
        &bowl,
        "same-length.dsql",
        "query Same {\n  title(limit 1) {\n    title\n  }\n}\n",
    )
    .await;
    assert_eq!(render_diagnostic_facts(&bowl).await, "");

    // Same byte length: `title` -> `titlx`.
    replace_source_text(&bowl, file, "    title\n", "    titlx\n").await;

    let diagnostics = render_diagnostic_facts(&bowl).await;
    assert!(
        diagnostics.contains("titlx"),
        "same-length body edits must re-run checks, got: {diagnostics:?}"
    );
}
