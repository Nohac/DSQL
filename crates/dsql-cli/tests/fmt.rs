//! `dsql fmt` end-to-end: a mixed project formats its dsql documents and
//! leaves TypeScript hosts untouched instead of parsing them as dsql.

use std::path::{Path, PathBuf};

use dsql_cli::commands::fmt_project;
use dsql_project::Project;

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("fixture dir");
    for entry in std::fs::read_dir(source).expect("fixture readable") {
        let entry = entry.expect("fixture entry");
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("fixture file copies");
        }
    }
}

#[tokio::test]
async fn mixed_projects_format_without_touching_hosts() {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
    let dir: PathBuf = std::env::temp_dir().join(format!("dsql-fmt-mixed-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fixture copy");
    }
    copy_tree(&source, &dir);

    let host_path = dir.join("src/components/TitlePanel.ts");
    let host_before = std::fs::read_to_string(&host_path).expect("host readable");

    let project = Project::load_from(&dir)
        .await
        .expect("fixture project loads");
    let clean = fmt_project(&project, false).await.expect("fmt succeeds");
    assert!(clean, "the canonical fixture needs no reformatting");

    let host_after = std::fs::read_to_string(&host_path).expect("host readable");
    assert_eq!(host_before, host_after, "hosts are not fmt's business");

    std::fs::remove_dir_all(&dir).ok();
}
