//! Work-shape guards: broad edits must not re-run every definition's
//! walks. Wall time is noisy; invocation counts (via the engine's
//! profiling counters) pin the intended shape directly.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::facts::DiagnosticsDemand;
use dsql_core::language_bowl;
use dsql_core::source::{FilePath, SourceText, insert_source};
use futures::executor::block_on;

use crate::imdb_catalog;

async fn runs_of(bowl: &Bowl, suffix: &str) -> u64 {
    bowl.profile_all()
        .await
        .into_iter()
        .find(|entry| entry.name.ends_with(suffix))
        .map(|entry| entry.runs)
        .unwrap_or_default()
}

async fn edit_file(bowl: &Bowl, path: &str, replace: (&str, &str)) {
    use bowl::Mut;
    let sources = bowl
        .scoop::<Query<(Entity, &FilePath)>>()
        .await
        .collect()
        .into_iter()
        .find(|(_, candidate)| candidate.0 == path)
        .map(|(entity, _)| entity);
    let target = sources.expect("edited file exists");
    let rows = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
    for (entity, source) in rows.collect() {
        if entity == target {
            let (from, to) = (replace.0.to_string(), replace.1.to_string());
            source
                .with_latest(move |text| {
                    let edited = text
                        .to_text()
                        .expect("editor text is resident")
                        .replace(&from, &to);
                    text.set_text(&edited);
                })
                .await;
        }
    }
    let _ = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
}

/// One query-file edit re-runs that file's checks, not the project's:
/// query bodies are not cross-file inputs, so the definition index (and
/// with it every other definition's walk) must stay untouched. A
/// fragment-file edit is the deliberate opposite — fragment bodies ARE
/// cross-file inputs, and every dependent walk re-runs.
#[test]
fn query_edits_rerun_one_definition_fragment_edits_rerun_all() {
    block_on(async {
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

        let baseline = runs_of(&bowl, "check_selections").await;

        edit_file(&bowl, "query-3.dsql", ("limit 1", "limit 2")).await;
        let after_query_edit = runs_of(&bowl, "check_selections").await;
        assert!(
            after_query_edit - baseline <= 2,
            "a query edit re-runs its own definition only, got {} extra runs",
            after_query_edit - baseline
        );

        edit_file(&bowl, "fragments.dsql", ("  id", "  title")).await;
        let after_fragment_edit = runs_of(&bowl, "check_selections").await;
        assert!(
            after_fragment_edit - after_query_edit >= FILES,
            "a fragment body edit re-runs every dependent walk, got {} runs",
            after_fragment_edit - after_query_edit
        );
    });
}
