//! Transactional publication (docs/spec/build-daemon.md): generation
//! allocation, idempotent republish, the publication lock, address
//! collisions, and prune retention.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dsql_core::source::ScopeImports;
use dsql_generate::publish::{
    ArtifactFamily, GenerationSnapshot, MatchLockMode, PublishedGeneration, SnapshotArtifact,
    SnapshotGroup, prune, publish as publish_generation,
    publish_with_deadline as publish_generation_with_deadline, sha256_hex,
};
use dsql_generate::{
    FilterMatchLock, GenerateError, LockedFilter, LockedFilterMatch, LockedPolicyReference,
    ProjectContract,
};

type Result<T> = std::result::Result<T, GenerateError>;

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
    let project_contract = ProjectContract::from_imports(&ScopeImports(
        std::collections::BTreeMap::from([("default".to_string(), Vec::new())]),
    ))
    .expect("default project contract");
    GenerationSnapshot {
        groups: vec![SnapshotGroup {
            name: "default".to_string(),
            imports: Vec::new(),
            generation_target: true,
            artifacts: artifacts
                .iter()
                .map(|artifact| artifact.id.clone())
                .collect(),
        }],
        artifacts,
        project_contract,
        filter_match_lock: dsql_generate::FilterMatchLock::empty(),
    }
}

fn snapshot_with_filter(name: &str, target: &str) -> GenerationSnapshot {
    let mut snapshot = snapshot(vec![artifact("A", "one")]);
    snapshot.filter_match_lock = FilterMatchLock {
        version: 1,
        filters: vec![LockedFilter {
            scope: "frontend".to_string(),
            defined_in: "shared".to_string(),
            name: name.to_string(),
            conditions: vec![LockedPolicyReference {
                scope: "shared".to_string(),
                name: "Allowed".to_string(),
            }],
            matches: vec![LockedFilterMatch {
                target: target.to_string(),
                fields: std::collections::BTreeMap::new(),
            }],
        }],
    };
    snapshot
}

fn build_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsql-publish-{test}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale dir");
    }
    dir
}

fn publish(build_dir: &Path, snapshot: &GenerationSnapshot) -> Result<PublishedGeneration> {
    publish_generation(
        build_dir,
        &build_dir.join("dsql.lock"),
        snapshot,
        MatchLockMode::Update,
    )
}

fn publish_with_deadline(
    build_dir: &Path,
    snapshot: &GenerationSnapshot,
    wait: Duration,
) -> Result<PublishedGeneration> {
    publish_generation_with_deadline(
        build_dir,
        &build_dir.join("dsql.lock"),
        snapshot,
        MatchLockMode::Update,
        wait,
    )
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
        &snapshot_with_filter("Visible", "public.users"),
        Duration::from_millis(200),
    )
    .expect_err("contended publication fails");
    assert!(
        matches!(error, GenerateError::PublicationLocked),
        "got {error}"
    );
    assert!(
        !dir.join("manifest.json").exists()
            && !dir.join("operations").exists()
            && !dir.join("dsql.lock").exists(),
        "a timed-out publication writes nothing"
    );

    drop(guard);
    publish(&dir, &snapshot(vec![artifact("A", "one")])).expect("publishes once released");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn match_lock_update_and_locked_modes_are_transactional() {
    let dir = build_dir("match-lock");
    let lock_path = dir.join("dsql.lock");
    let desired = snapshot_with_filter("Visible", "public.users");

    let first = publish_generation(&dir, &lock_path, &desired, MatchLockMode::Update)
        .expect("unlocked publication writes the match lock");
    let canonical = std::fs::read_to_string(&lock_path).expect("lock written");
    assert_eq!(
        first.filter_match_lock_hash,
        Some(sha256_hex(canonical.as_bytes()))
    );

    let noncanonical = indoc::indoc! {r#"
        version: 1
        filters:
          - name: Visible
            scope: frontend
            defined_in: shared
            matches:
              - target: public.users
            conditions:
              - name: Allowed
                scope: shared
    "#};
    std::fs::write(&lock_path, noncanonical).expect("write semantically equal lock");
    let accepted = publish_generation(&dir, &lock_path, &desired, MatchLockMode::Locked)
        .expect("locked mode compares canonical semantics");
    assert_eq!(
        accepted.filter_match_lock_hash,
        Some(sha256_hex(noncanonical.as_bytes()))
    );
    assert_eq!(
        std::fs::read_to_string(&lock_path).expect("accepted lock remains"),
        noncanonical,
        "locked mode never canonicalizes the file",
    );

    let stale = snapshot_with_filter("Visible", "public.posts");
    let error = publish_generation(&dir, &lock_path, &stale, MatchLockMode::Locked)
        .expect_err("stale lock blocks publication");
    assert!(
        matches!(error, GenerateError::MatchLock { .. })
            && error
                .to_string()
                .contains("- frontend <- shared::Visible: public.users")
            && error
                .to_string()
                .contains("+ frontend <- shared::Visible: public.posts"),
        "unexpected stale-lock error: {error}",
    );
    assert!(
        !dir.join(format!("manifest.{}.json", accepted.generation_id + 1))
            .exists(),
        "a stale lock writes no next generation",
    );
    no_temp_files(&dir);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn match_lock_empty_and_newer_version_edges_fail_closed() {
    let dir = build_dir("match-lock-edges");
    let lock_path = dir.join("dsql.lock");
    let empty = snapshot(Vec::new());

    publish_generation(&dir, &lock_path, &empty, MatchLockMode::Locked)
        .expect("locked empty state accepts an absent lock");
    assert!(!lock_path.exists());

    std::fs::write(&lock_path, "version: 1\nfilters: []\n").expect("write empty lock");
    let error = publish_generation(&dir, &lock_path, &empty, MatchLockMode::Locked)
        .expect_err("an empty lock file is stale; canonical state is absence");
    assert!(matches!(error, GenerateError::MatchLock { .. }));
    publish_generation(&dir, &lock_path, &empty, MatchLockMode::Update)
        .expect("unlocked mode removes the empty lock");
    assert!(!lock_path.exists());

    let desired = snapshot_with_filter("Visible", "public.users");
    std::fs::write(&lock_path, "this: is: not: yaml\n").expect("write malformed lock");
    let error = publish_generation(&dir, &lock_path, &desired, MatchLockMode::Locked)
        .expect_err("locked mode refuses a malformed lock");
    assert!(
        matches!(error, GenerateError::MatchLock { .. })
            && error.to_string().contains("lock is malformed"),
        "unexpected malformed-lock error: {error}",
    );
    publish_generation(&dir, &lock_path, &desired, MatchLockMode::Update)
        .expect("update mode replaces a malformed supported-version lock");
    assert!(
        std::fs::read_to_string(&lock_path)
            .expect("replacement lock readable")
            .contains("Visible")
    );

    let newer = "version: 2\nfilters: []\n";
    std::fs::write(&lock_path, newer).expect("write newer lock");
    let error = publish_generation(&dir, &lock_path, &desired, MatchLockMode::Update)
        .expect_err("an older updater must not overwrite a newer lock");
    assert!(
        matches!(error, GenerateError::MatchLock { .. })
            && error.to_string().contains("version 2 is newer"),
        "unexpected newer-version error: {error}",
    );
    assert_eq!(
        std::fs::read_to_string(&lock_path).expect("newer lock remains"),
        newer,
    );
    assert!(
        !dir.join("manifest.4.json").exists(),
        "newer-version refusal happens before generation allocation",
    );

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
