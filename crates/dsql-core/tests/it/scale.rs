//! Work-shape guards: broad edits must not re-run every definition's
//! walks. Wall time is noisy; invocation counts (via the engine's
//! profiling counters) pin the intended shape directly.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::TableRef;
use dsql_core::facts::DiagnosticsDemand;
use dsql_core::language_bowl;
use dsql_core::source::{FilePath, insert_source};

use crate::{imdb_catalog, replace_source_text};

async fn runs_of(bowl: &Bowl, suffix: &str) -> u64 {
    bowl.profile_all()
        .await
        .into_iter()
        .find(|entry| entry.name.ends_with(suffix))
        .map(|entry| entry.runs)
        .unwrap_or_default()
}

async fn edit_file(bowl: &Bowl, path: &str, replace: (&str, &str)) {
    let sources = bowl
        .scoop::<Query<(Entity, &FilePath)>>()
        .await
        .collect()
        .into_iter()
        .find(|(_, candidate)| candidate.0 == path)
        .map(|(entity, _)| entity);
    let target = sources.expect("edited file exists");
    replace_source_text(bowl, target, replace.0, replace.1).await;
    let _ = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
}

/// Exact field-resolution checks follow changed fields, not definition-wide
/// ambient invalidation. A fragment body is checked under its own declared
/// target; callers retain separate spread-site compatibility checks.
#[tokio::test]
async fn selection_checks_follow_changed_resolution_pairs_only() {
    let bowl = language_bowl().await;
    dsql_core::catalog::insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;

    const FILES: u64 = 20;
    for index in 0..FILES {
        insert_source(
            &bowl,
            format!("query-{index}.dsql"),
            &format!("query Q{index} {{\n  title(limit 1) {{\n    id\n  }}\n}}\n"),
        )
        .await;
    }
    insert_source(
        &bowl,
        "fragments.dsql",
        "fragment Bits on title {\n  id\n}\n",
    )
    .await;
    let _ = bowl.scoop::<Query<(Entity, &FilePath)>>().await;

    let baseline = runs_of(&bowl, "check_resolved_selection").await;

    edit_file(&bowl, "query-3.dsql", ("limit 1", "limit 2")).await;
    let after_query_edit = runs_of(&bowl, "check_resolved_selection").await;
    assert_eq!(
        after_query_edit - baseline,
        2,
        "syntax and resolution revisions converge on the one changed pair"
    );

    edit_file(&bowl, "fragments.dsql", ("  id", "  title")).await;
    let after_fragment_edit = runs_of(&bowl, "check_resolved_selection").await;
    assert_eq!(
        after_fragment_edit - after_query_edit,
        2,
        "syntax and resolution revisions converge on the one local fragment pair"
    );
}

/// Catalog replacements rerun the resolution systems because the catalog is
/// one tracked singleton. Fingerprint cutoff on the normalized clause result
/// must prevent an unchanged sibling type contract from reaching its check.
#[tokio::test]
async fn clause_checks_follow_changed_catalog_type_contracts_only() {
    let bowl = language_bowl().await;
    let mut catalog = imdb_catalog();
    dsql_core::catalog::insert_catalog(&bowl, catalog.clone()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;

    let first = insert_source(
        &bowl,
        "first.dsql",
        "query First {\n  title(where .id == 1) { id }\n}\n",
    )
    .await;
    insert_source(
        &bowl,
        "second.dsql",
        "query Second {\n  title(where .title == \"x\") { title }\n}\n",
    )
    .await;
    let _ = bowl
        .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
        .await;
    let baseline = runs_of(&bowl, "check_resolved_clause").await;

    replace_source_text(&bowl, first, ".id == 1", ".id == 2").await;
    let _ = bowl
        .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
        .await;
    assert_eq!(
        runs_of(&bowl, "check_resolved_clause").await - baseline,
        1,
        "a content-only clause edit reruns only its exact syntax-resolution pair"
    );
    let before_catalog = runs_of(&bowl, "check_resolved_clause").await;

    let id_column = catalog
        .table_ref_for(TableRef::parse("title"))
        .and_then(|table| {
            table.columns.iter().find_map(|column| {
                catalog
                    .column_by_id(*column)
                    .filter(|column| column.name == "id")
                    .map(|column| column.id)
            })
        })
        .expect("the imdb title table has an id column");
    let type_id = catalog
        .column_by_id(id_column)
        .expect("the id column remains in the catalog")
        .type_id;
    catalog.types[type_id.0]
        .capabilities
        .description
        .push_str(" (probe)");
    dsql_core::catalog::insert_catalog(&bowl, catalog).await;
    let _ = bowl
        .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
        .await;

    assert_eq!(
        runs_of(&bowl, "check_resolved_clause").await - before_catalog,
        1,
        "only the clause whose resolved value contract changed reruns"
    );
}

#[tokio::test]
async fn fragment_body_materialization_wakes_only_dependent_roots() {
    let bowl = language_bowl().await;
    dsql_core::catalog::insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;

    const UNRELATED: u64 = 20;
    for index in 0..UNRELATED {
        insert_source(
            &bowl,
            format!("unrelated-{index}.dsql"),
            &format!("query Unrelated{index} {{ title(limit 1) {{ id }} }}\n"),
        )
        .await;
    }
    insert_source(
        &bowl,
        "dependent.dsql",
        "fragment Bits on title { id }\nquery Dependent { title(limit 1) { ...Bits } }\n",
    )
    .await;
    let _ = bowl
        .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
        .await;

    let materialize_before = runs_of(&bowl, "materialize_expansion_bodies").await;
    let checks_before = runs_of(&bowl, "residual_definition_checks").await;
    edit_file(
        &bowl,
        "dependent.dsql",
        ("Bits on title { id", "Bits on title { title"),
    )
    .await;
    let _ = bowl
        .scoop::<Query<(Entity, &dsql_core::facts::Diagnostic)>>()
        .await;
    let materialize_delta =
        runs_of(&bowl, "materialize_expansion_bodies").await - materialize_before;
    let checks_delta = runs_of(&bowl, "residual_definition_checks").await - checks_before;
    assert_eq!(
        materialize_delta, 2,
        "only the dependent occurrence body follows syntax and resolution convergence"
    );
    assert_eq!(
        checks_delta, 6,
        "the fragment root and its dependent query converge without waking twenty unrelated roots"
    );
}
