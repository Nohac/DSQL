//! Transactional publication (docs/spec/build-daemon.md): generation
//! allocation, idempotent republish, the publication lock, address
//! collisions, and prune retention.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dsql_generate::GenerateError;
use dsql_generate::publish::{
    ArtifactFamily, GenerationSnapshot, SnapshotArtifact, SnapshotGroup, prune, publish,
    publish_with_deadline, sha256_hex,
};

fn artifact(name: &str, body: &str) -> SnapshotArtifact {
    let serialized = format!("{{\"name\":\"{name}\",\"body\":\"{body}\"}}");
    let hash = sha256_hex(serialized.as_bytes());
    SnapshotArtifact {
        id: format!("default/operation/{name}"),
        family: ArtifactFamily::Operation,
        kind: "query".to_string(),
        scope: "default".to_string(),
        name: name.to_string(),
        serialized,
        hash,
        source: format!("queries/{name}.dsql"),
    }
}

fn snapshot(artifacts: Vec<SnapshotArtifact>) -> GenerationSnapshot {
    GenerationSnapshot {
        groups: vec![SnapshotGroup {
            name: "default".to_string(),
            imports: Vec::new(),
            artifacts: artifacts
                .iter()
                .map(|artifact| artifact.id.clone())
                .collect(),
        }],
        artifacts,
    }
}

fn build_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsql-publish-{test}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    dir
}

fn no_temp_files(dir: &Path) {
    for entry in walk(dir) {
        let name = entry.file_name().unwrap_or_default().to_string_lossy();
        assert!(
            !name.contains(".tmp-"),
            "no temp files may survive publication: {}",
            entry.display()
        );
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return paths;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_dir() {
            paths.extend(walk(&path));
        } else {
            paths.push(path);
        }
    }
    paths
}

/// Ids advance monotonically, identical content republishes as a no-op,
/// and stranded immutable manifests skip their ids instead of reusing
/// them.
#[test]
fn generation_ids_never_recycle() {
    let dir = build_dir("ids");

    let first = publish(&dir, &snapshot(vec![artifact("A", "one")])).expect("first publish");
    assert_eq!(first.generation_id, 1);
    no_temp_files(&dir);

    // Republishing identical content still commits a fresh generation
    // (no-op detection is the daemon's reconciliation, not publication),
    // but the content-addressed artifact files themselves are skipped.
    let again = publish(&dir, &snapshot(vec![artifact("A", "one")])).expect("republish");
    assert_eq!(again.generation_id, 2, "every publication is a generation");
    assert!(
        again
            .written
            .iter()
            .all(|path| !path.to_string_lossy().contains("operations/")),
        "identical artifacts are not rewritten"
    );

    // A stranded manifest (crash between immutable write and pointer
    // commit) must skip its id.
    std::fs::write(dir.join("manifest.7.json"), "{}").expect("stranded manifest");
    let second = publish(&dir, &snapshot(vec![artifact("A", "two")])).expect("second publish");
    assert_eq!(
        second.generation_id, 8,
        "max-on-disk rule skips stranded ids"
    );

    // A malformed pointer never recycles ids either.
    std::fs::write(dir.join("manifest.json"), "not json").expect("corrupt pointer");
    let third = publish(&dir, &snapshot(vec![artifact("A", "three")])).expect("third publish");
    assert_eq!(third.generation_id, 9, "ids advance from the disk scan");
    no_temp_files(&dir);

    std::fs::remove_dir_all(&dir).ok();
}

/// A held lock bounds publication with `PublicationLocked` and leaves the
/// tree untouched.
#[test]
fn contended_publication_fails_bounded_and_writes_nothing() {
    let dir = build_dir("contended");
    std::fs::create_dir_all(&dir).expect("build dir");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))
        .expect("lock file");
    let mut lock = fd_lock::RwLock::new(lock_file);
    let guard = lock.write().expect("lock held by the test");

    let error = publish_with_deadline(
        &dir,
        &snapshot(vec![artifact("A", "one")]),
        Duration::from_millis(200),
    )
    .expect_err("contended publication fails");
    assert!(
        matches!(error, GenerateError::PublicationLocked),
        "got {error}"
    );
    assert!(
        !dir.join("manifest.json").exists() && !dir.join("operations").exists(),
        "a timed-out publication writes nothing"
    );

    drop(guard);
    publish(&dir, &snapshot(vec![artifact("A", "one")])).expect("publishes once released");

    std::fs::remove_dir_all(&dir).ok();
}

/// An existing file at an artifact's address with different bytes is a
/// hard error, never a silent overwrite.
#[test]
fn address_collisions_refuse_to_overwrite() {
    let dir = build_dir("collision");
    let one = artifact("A", "one");
    let address_path = {
        // Compute where the artifact will land, then plant different bytes.
        let published = publish(&dir, &snapshot(vec![one.clone()])).expect("first publish");
        let path = published
            .written
            .iter()
            .find(|path| path.to_string_lossy().contains("operations/"))
            .expect("operation written")
            .clone();
        std::fs::remove_dir_all(&dir).expect("reset");
        path
    };
    std::fs::create_dir_all(address_path.parent().expect("parent")).expect("dirs");
    std::fs::write(&address_path, "different bytes").expect("planted");

    let error = publish(&dir, &snapshot(vec![one])).expect_err("collision refuses");
    assert!(
        matches!(error, GenerateError::AddressCollision { .. }),
        "got {error}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Pruning retains the current and predecessor generations, removes
/// older ones, and skips entirely when superseded.
#[test]
fn pruning_retains_current_and_predecessor() {
    let dir = build_dir("prune");

    let first = publish(&dir, &snapshot(vec![artifact("A", "one")])).expect("gen 1");
    let second = publish(&dir, &snapshot(vec![artifact("A", "two")])).expect("gen 2");
    let third = publish(&dir, &snapshot(vec![artifact("A", "three")])).expect("gen 3");

    prune(&dir, &third);
    assert!(
        !dir.join("manifest.1.json").exists(),
        "generation 1 is pruned"
    );
    assert!(
        dir.join("manifest.2.json").exists() && dir.join("manifest.3.json").exists(),
        "current and predecessor manifests are retained"
    );
    let survivors: Vec<String> = walk(&dir.join("operations"))
        .into_iter()
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(
        survivors.len(),
        2,
        "gen-2 and gen-3 artifacts survive, gen-1's is pruned: {survivors:?}"
    );

    // A superseded publisher must skip pruning: pruning for gen 2 while
    // the pointer names gen 3 does nothing.
    prune(&dir, &second);
    assert!(
        dir.join("manifest.2.json").exists() && dir.join("manifest.3.json").exists(),
        "superseded pruning is a no-op"
    );

    let _ = first;
    std::fs::remove_dir_all(&dir).ok();
}
