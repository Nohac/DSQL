//! Definition facts: queries and fragments lower into `DefDecl` entities,
//! and duplicate fragment names surface as demand-gated diagnostics.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::entities::definition::{DefDecl, FragmentTarget};
use dsql_core::facts::{Diagnostic, DiagnosticsDemand};
use dsql_core::register_language;
use dsql_core::source::insert_source;
use futures::executor::block_on;

use crate::{fixture, render_diagnostic_facts};

async fn language_bowl() -> Bowl {
    let db = Bowl::new();
    register_language(&db).await;
    db
}

/// Renders every definition fact, sorted for stability.
async fn render_def_facts(db: &Bowl) -> String {
    let rows = db.scoop::<Query<(Entity, &DefDecl)>>().await;
    let targets = db.scoop::<Query<(Entity, &FragmentTarget)>>().await;
    let targets = targets.collect();

    let mut lines: Vec<String> = rows
        .collect()
        .into_iter()
        .map(|(entity, decl)| {
            let target = targets
                .iter()
                .find(|(target_entity, _)| *target_entity == entity)
                .map(|(_, target)| format!(" on {} [{}..{}]", target.name, target.span.start, target.span.end))
                .unwrap_or_default();
            format!(
                "{} {} [{}..{}] name[{}..{}]{target}",
                decl.kind, decl.name, decl.span.start, decl.span.end,
                decl.name_span.start, decl.name_span.end,
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[test]
fn definitions_lower_into_facts() {
    block_on(async {
        let db = language_bowl().await;

        insert_source(
            &db,
            "valid/imdb-fragment-spread.dsql",
            &fixture("valid/imdb-fragment-spread.dsql"),
        )
        .await;

        insta::assert_snapshot!(render_def_facts(&db).await);
    });
}

#[test]
fn duplicate_fragments_are_reported_on_demand() {
    block_on(async {
        let db = language_bowl().await;

        insert_source(
            &db,
            "dupes.dsql",
            "fragment F on title {\n  id\n}\nfragment F on title {\n  title\n}\nquery Q {\n  title {\n    ...F\n  }\n}\n",
        )
        .await;

        let undemanded = db.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(undemanded, 0, "duplicate checks must not run undemanded");

        db.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        insta::assert_snapshot!(render_diagnostic_facts(&db).await);
    });
}

#[test]
fn duplicate_fragments_in_different_files_are_allowed() {
    block_on(async {
        let db = language_bowl().await;

        insert_source(&db, "a.dsql", "fragment F on title {\n  id\n}\n").await;
        insert_source(&db, "b.dsql", "fragment F on title {\n  id\n}\n").await;

        db.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        let diagnostics = db.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(
            diagnostics, 0,
            "fragment duplicate scope is per file (dsql-poc FragmentMap semantics)"
        );
    });
}
