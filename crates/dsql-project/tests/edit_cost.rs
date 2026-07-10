//! Perf probe: what one keystroke costs. Loads a project, arms the LSP's
//! demands, then toggles one byte inside an embedded region and settles,
//! printing the per-edit wall time — the number to watch before landing
//! engine-touching changes. Run with `--ignored`; point
//! `EDIT_COST_PROJECT`/`EDIT_COST_HOST` at a real project (and raise
//! `EDIT_COST_ROUNDS`) to reproduce editor-session numbers or to profile
//! under callgrind.

use bowl::{Entity, Eq as BowlEq, Mut, Query, Singleton, Where};
use dsql_core::facts::{DiagnosticsDemand, VariablesDemand};
use dsql_core::service::TokensDemand;
use dsql_core::source::{FilePath, OpenBuffer, SourceText};
use dsql_project::{Project, open_project_bowl};
use std::path::Path;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "perf probe for single-keystroke settle cost; run explicitly"]
async fn edit_settle_cost() {
    let (root, host_rel): (std::path::PathBuf, String) = match std::env::var("EDIT_COST_PROJECT") {
        Ok(dir) => (
            dir.into(),
            std::env::var("EDIT_COST_HOST").expect("EDIT_COST_HOST set with the project"),
        ),
        Err(_) => (
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/scoped"),
            "src/components/TitlePanel.ts".to_string(),
        ),
    };
    let rounds: usize = std::env::var("EDIT_COST_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    let project = Project::load_from(&root).await.expect("project loads");
    let bowl = open_project_bowl(&project).await.expect("bowl assembles");
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    bowl.insert((Singleton::<TokensDemand>::new(), TokensDemand))
        .await;

    // Scoped: a live QueryResult holds read guards on every row it
    // matched, and a later `with_latest` on one of those rows spins
    // forever waiting for the cell to free.
    let (host, host_path) = {
        let sources = bowl
            .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
            .await;
        sources
            .collect()
            .into_iter()
            .find(|(_, path, _)| path.0.ends_with(&host_rel))
            .map(|(entity, path, _)| (entity, path.0.clone()))
            .expect("host in project")
    };
    bowl.entity(host).insert((OpenBuffer,)).await;
    // Full warm-up settle before timing.
    let _ = bowl.scoop::<Query<(Entity, &FilePath)>>().await;

    // The edit target must sit inside an embedded region so every round
    // reparses, re-resolves, and re-checks it.
    let anchor = "title";
    for round in 0..rounds {
        let started = Instant::now();
        let sources = bowl
            .scoop::<Query<(Entity, Mut<SourceText>), Where<BowlEq<FilePath>>>>()
            .args(FilePath(host_path.clone()))
            .await;
        for (_, source) in sources.collect() {
            source
                .with_latest(move |text| {
                    let current = text.to_text();
                    let Some(at) = current.find(anchor) else {
                        panic!("anchor not in host");
                    };
                    let edited = if round % 2 == 0 {
                        format!(
                            "{}X{}",
                            &current[..at + anchor.len()],
                            &current[at + anchor.len()..]
                        )
                    } else {
                        current.replacen("titleX", "title", 1)
                    };
                    text.set_text(&edited);
                })
                .await;
        }
        drop(sources);
        let _ = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
        println!("edit {round}: {:?}", started.elapsed());
    }

    // Per-system planning/run attribution for the whole session; the
    // per-edit share is what to optimize.
    if std::env::var("EDIT_COST_PROFILE").is_ok() {
        let mut profile = bowl.profile_all().await;
        profile.sort_by_key(|entry| std::cmp::Reverse(entry.plan_nanos));
        println!("system, runs, plan_ms, run_ms");
        for entry in profile.iter().take(25) {
            println!(
                "{}, {}, {:.1}, {:.1}",
                entry.name,
                entry.runs,
                entry.plan_nanos as f64 / 1e6,
                entry.run_nanos as f64 / 1e6
            );
        }
    }
}
