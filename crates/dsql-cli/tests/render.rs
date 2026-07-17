//! Diagnostic rendering: findings inside embedded regions render under the
//! host file with excerpts in host coordinates.

use bowl::Singleton;
use dsql_cli::render::{collect_diagnostics, render_themed};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::DiagnosticsDemand;
use dsql_core::language_bowl;
use dsql_core::source::{arm_analysis_residency, insert_embedding_source};
use miette::GraphicalTheme;

#[tokio::test]
async fn embedded_findings_render_against_the_host_file() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;

    let host = "import { dsql } from \"./dsql\";\n\nexport const q = dsql`\nquery Q {\n  users {\n    id\n    nonexistent\n  }\n}\n`;\n";
    insert_embedding_source(&bowl, "src/users.ts", host, "typescript").await;

    let diagnostics = collect_diagnostics(&bowl).await;
    let rendered: Vec<String> = diagnostics
        .iter()
        .map(|diagnostic| render_themed(diagnostic, GraphicalTheme::unicode_nocolor()))
        .collect();
    insta::assert_snapshot!(rendered.join("\n"));
}

const BROKEN_HOST: &str = "import { dsql } from \"./dsql\";\n\nexport const q = dsql`\nquery Q {\n  users {\n    id\n    nonexistent\n  }\n}\n`;\n";

/// With analysis residency the host's rope is evicted after extraction;
/// excerpts recover by re-reading the file from disk, accepted only when
/// it still matches the stored content hash.
#[tokio::test]
async fn evicted_host_excerpts_recover_from_disk() {
    let dir = std::env::temp_dir().join(format!("dsql-render-evicted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let host_path = dir.join("users.ts");
    std::fs::write(&host_path, BROKEN_HOST).expect("host written");

    let bowl = language_bowl().await;
    arm_analysis_residency(&bowl).await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insert_embedding_source(
        &bowl,
        host_path.to_str().expect("utf8 path"),
        BROKEN_HOST,
        "typescript",
    )
    .await;

    let diagnostics = collect_diagnostics(&bowl).await;
    assert_eq!(diagnostics.len(), 1);
    let rendered = render_themed(&diagnostics[0], GraphicalTheme::unicode_nocolor());
    assert!(
        rendered.contains("nonexistent") && rendered.contains("users.ts"),
        "the excerpt recovers from disk, got:\n{rendered}"
    );
    assert!(
        rendered.contains("│"),
        "a source snippet renders, got:\n{rendered}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A host that changed on disk after analysis must not render a stale
/// excerpt: the diagnostic still reports, snippet-free.
#[tokio::test]
async fn changed_on_disk_hosts_render_without_stale_excerpts() {
    let dir = std::env::temp_dir().join(format!("dsql-render-changed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let host_path = dir.join("users.ts");
    std::fs::write(&host_path, BROKEN_HOST).expect("host written");

    let bowl = language_bowl().await;
    arm_analysis_residency(&bowl).await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insert_embedding_source(
        &bowl,
        host_path.to_str().expect("utf8 path"),
        BROKEN_HOST,
        "typescript",
    )
    .await;
    // Settle (and evict) before the disk content diverges.
    let diagnostics = collect_diagnostics(&bowl).await;
    assert_eq!(diagnostics.len(), 1);

    std::fs::write(&host_path, "// rewritten\n").expect("host rewritten");
    let diagnostics = collect_diagnostics(&bowl).await;
    assert_eq!(diagnostics.len(), 1, "the finding itself is unaffected");
    let rendered = render_themed(&diagnostics[0], GraphicalTheme::unicode_nocolor());
    assert!(
        rendered.contains("unknown column") || rendered.contains("nonexistent"),
        "the message still reports, got:\n{rendered}"
    );
    assert!(
        rendered.contains("users.ts") && rendered.contains("["),
        "the location survives without an excerpt, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("dsql`") && !rendered.contains("// rewritten"),
        "neither stale nor mismatched text renders, got:\n{rendered}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
