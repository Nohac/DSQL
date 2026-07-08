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
    let bowl = Bowl::new();
    register_language(&bowl).await;
    bowl
}

/// Renders every definition fact, sorted for stability.
async fn render_def_facts(bowl: &Bowl) -> String {
    let rows = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let targets = bowl.scoop::<Query<(Entity, &FragmentTarget)>>().await;
    let targets = targets.collect();

    let mut lines: Vec<String> = rows
        .collect()
        .into_iter()
        .map(|(entity, decl)| {
            let target = targets
                .iter()
                .find(|(target_entity, _)| *target_entity == entity)
                .map(|(_, target)| {
                    format!(
                        " on {} [{}..{}]",
                        target.name, target.span.start, target.span.end
                    )
                })
                .unwrap_or_default();
            format!(
                "{} {} [{}..{}] name[{}..{}]{target}",
                decl.kind,
                decl.name,
                decl.span.start,
                decl.span.end,
                decl.name_span.start,
                decl.name_span.end,
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[test]
fn definitions_lower_into_facts() {
    block_on(async {
        let bowl = language_bowl().await;

        insert_source(
            &bowl,
            "valid/imdb-fragment-spread.dsql",
            &fixture("valid/imdb-fragment-spread.dsql"),
        )
        .await;

        insta::assert_snapshot!(render_def_facts(&bowl).await);
    });
}

#[test]
fn duplicate_fragments_are_reported_on_demand() {
    block_on(async {
        let bowl = language_bowl().await;

        insert_source(
            &bowl,
            "dupes.dsql",
            "fragment F on title {\n  id\n}\nfragment F on title {\n  title\n}\nquery Q {\n  title {\n    ...F\n  }\n}\n",
        )
        .await;

        let undemanded = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(undemanded, 0, "duplicate checks must not run undemanded");

        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn duplicate_fragments_across_files_in_one_scope_are_reported() {
    block_on(async {
        let bowl = language_bowl().await;

        insert_source(&bowl, "a.dsql", "fragment F on title {\n  id\n}\n").await;
        insert_source(&bowl, "b.dsql", "fragment F on title {\n  id\n}\n").await;

        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        let diagnostics = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
        assert_eq!(
            diagnostics, 1,
            "one scope resolves fragments across files, so the names collide"
        );
    });
}
