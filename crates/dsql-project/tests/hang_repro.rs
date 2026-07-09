//! Repro hunt for the LSP stall: the editor's didOpen+hover sequence as
//! plain bowl operations — scoop, external marker insert, request take —
//! looped. Run with `--ignored`; a hang here is an engine repro.

use bowl::{Entity, Query, Singleton};
use dsql_core::facts::{DiagnosticsDemand, VariablesDemand};
use dsql_core::service::{HoverInfo, HoverRequest, Position};
use dsql_core::source::{FilePath, OpenBuffer, SourceText};
use dsql_project::{Project, open_project_bowl};
use std::path::Path;

#[tokio::test]
#[ignore = "stress repro for the didOpen+hover stall; run explicitly"]
async fn did_open_then_hover_never_stalls() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/scoped");
    let project = Project::load_from(&fixture).await.expect("fixture loads");

    for round in 0..200 {
        let bowl = open_project_bowl(&project).await.expect("bowl assembles");
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
            .await;

        // didOpen: resolve the host entity (scoop settles the project),
        // stamp the open-buffer marker externally.
        let sources = bowl
            .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
            .await;
        let host = sources
            .collect()
            .into_iter()
            .find(|(_, path, _)| path.0.ends_with("TitlePanel.ts"))
            .map(|(entity, _, _)| entity)
            .expect("host in project");
        bowl.entity(host).insert((OpenBuffer,)).await;

        // hover immediately after, host coordinates.
        let host_text = std::fs::read_to_string(fixture.join("src/components/TitlePanel.ts"))
            .expect("host readable");
        let offset = host_text.find("TitleBits").expect("spread in host");
        let path = fixture
            .join("src/components/TitlePanel.ts")
            .display()
            .to_string();
        let answer = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            bowl.insert((HoverRequest, FilePath(path), Position { offset }))
                .await
                .bind()
                .take::<HoverInfo>()
                .await
        })
        .await;
        assert!(answer.is_ok(), "round {round}: take stalled >20s");
    }
}

/// The same two sequences racing on a multi-thread runtime, the way
/// tower-lsp actually runs handlers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "stress repro for the didOpen+hover stall; run explicitly"]
async fn concurrent_open_and_hover_never_stall() {
    // Point HANG_REPRO_PROJECT at a real project (and HANG_REPRO_HOST at a
    // host file within it) to widen the race window beyond the fixture.
    let (root, host_rel): (std::path::PathBuf, String) = match std::env::var("HANG_REPRO_PROJECT") {
        Ok(dir) => (
            dir.into(),
            std::env::var("HANG_REPRO_HOST").expect("HANG_REPRO_HOST set with the project"),
        ),
        Err(_) => (
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixture/scoped"),
            "src/components/TitlePanel.ts".to_string(),
        ),
    };
    let project = Project::load_from(&root).await.expect("project loads");
    let host_path = root.join(&host_rel).display().to_string();
    let host_text = std::fs::read_to_string(root.join(&host_rel)).expect("host readable");
    let offset = host_text
        .find("fragment ")
        .or_else(|| host_text.find("query "))
        .expect("template in host")
        + "fragment ".len()
        + 2;

    for round in 0..100 {
        let bowl = open_project_bowl(&project).await.expect("bowl assembles");
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
            .await;

        let open_bowl = bowl.clone();
        let open_path = host_path.clone();
        let open = tokio::spawn(async move {
            let sources = open_bowl
                .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
                .await;
            let host = sources
                .collect()
                .into_iter()
                .find(|(_, path, _)| path.0 == open_path)
                .map(|(entity, _, _)| entity)
                .expect("host in project");
            open_bowl.entity(host).insert((OpenBuffer,)).await;
            // publish-ish: another scoop after the external insert.
            let _ = open_bowl
                .scoop::<Query<(Entity, &dsql_core::facts::Severity)>>()
                .await;
        });

        let hover_bowl = bowl.clone();
        let hover_path = host_path.clone();
        let hover = tokio::spawn(async move {
            hover_bowl
                .insert((HoverRequest, FilePath(hover_path), Position { offset }))
                .await
                .bind()
                .take::<HoverInfo>()
                .await
        });

        let both = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let _ = open.await;
            let _ = hover.await;
        })
        .await;
        assert!(both.is_ok(), "round {round}: stalled >20s");
    }
}
