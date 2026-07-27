//! Transactional publication of a generation snapshot
//! (docs/spec/build-daemon.md, Transactionality): content-addressed
//! artifact files, an immutable per-generation manifest, and a fixed
//! `manifest.json` pointer committed last by atomic rename — all under an
//! advisory publication lock shared by every writer, with pruning as a
//! separate best-effort post-commit step.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dsql_metadata::{
    BUILD_MANIFEST_VERSION, BuildManifest, FragmentManifestEntry, OperationManifestEntry,
};

use crate::layout::{MANIFEST_FILE, artifact_file_name, generation_manifest_file};
use crate::pipeline::{GenerateError, Result};
pub use crate::snapshot::{
    ArtifactFamily, GenerationSnapshot, SnapshotArtifact, SnapshotGroup, artifact_address,
    sha256_hex,
};
use crate::{FILTER_MATCH_LOCK_VERSION, FilterMatchLock};

/// A committed generation: its id, the immutable manifest (path and
/// serialized document), the pointer, and what publication wrote.
/// Survives a host-generator failure so the error can name the committed
/// generation and pruning can still run.
#[derive(Debug, Clone)]
pub struct PublishedGeneration {
    pub generation_id: u64,
    /// The immutable `manifest.<id>.json`.
    pub manifest_path: PathBuf,
    /// The manifest document, exactly as written.
    pub manifest_json: String,
    /// The fixed `manifest.json` pointer.
    pub current_manifest_path: PathBuf,
    /// Files written this publication (unchanged artifacts are skipped).
    pub written: Vec<PathBuf>,
    /// SHA-256 of the accepted lock bytes, or `None` when the canonical lock
    /// state is an absent file.
    pub filter_match_lock_hash: Option<String>,
    /// The generation the pointer named before this commit.
    predecessor: Option<u64>,
}

/// Native filter match-lock behavior selected by a production writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchLockMode {
    /// Canonicalize the current resolved matches onto disk.
    Update,
    /// Require an existing semantic match without modifying it.
    Locked,
}

/// Result of reconciling one canonical match lock under the publication lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchLockStatus {
    /// Whether the lock file was written, replaced, or removed.
    pub changed: bool,
    /// Hash of the accepted bytes, or `None` for an absent empty lock.
    pub content_hash: Option<String>,
}

#[derive(facet::Facet)]
struct MatchLockVersion {
    version: u32,
}

/// Minimal publication pointer header used only to avoid reusing generation
/// identifiers. This is not an artifact compatibility reader.
#[derive(facet::Facet)]
struct PointerManifest {
    #[facet(default)]
    version: u32,
    #[facet(default, rename = "generationId")]
    generation_id: u64,
}

const LOCK_FILE: &str = ".lock";
const LOCK_DEADLINE: Duration = Duration::from_secs(3);
const LOCK_POLL: Duration = Duration::from_millis(50);

/// Acquires the publication lock, polling to a fixed deadline.
fn acquire_lock(build_dir: &Path) -> Result<(std::fs::File, PathBuf)> {
    std::fs::create_dir_all(build_dir).map_err(|source| GenerateError::Write {
        path: build_dir.to_path_buf(),
        source,
    })?;
    let lock_path = build_dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|source| GenerateError::Write {
            path: lock_path.clone(),
            source,
        })?;
    Ok((file, lock_path))
}

/// Runs `body` while holding the exclusive publication lock; times out
/// with [`GenerateError::PublicationLocked`].
fn with_publication_lock<T>(build_dir: &Path, body: impl FnOnce() -> Result<T>) -> Result<T> {
    with_publication_lock_deadline(build_dir, LOCK_DEADLINE, body)
}

fn with_publication_lock_deadline<T>(
    build_dir: &Path,
    wait: Duration,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let (file, lock_path) = acquire_lock(build_dir)?;
    let mut lock = fd_lock::RwLock::new(file);
    let deadline = Instant::now() + wait;
    loop {
        match lock.try_write() {
            Ok(guard) => {
                let outcome = body();
                drop(guard);
                return outcome;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(GenerateError::PublicationLocked);
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(source) => {
                return Err(GenerateError::Write {
                    path: lock_path,
                    source,
                });
            }
        }
    }
}

/// Writes `content` to `path` atomically: a unique sibling temp file
/// (`create_new`), write + flush + sync, then rename over the target.
/// The temp file is removed on any failure.
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| GenerateError::Write {
        path: parent.to_path_buf(),
        source,
    })?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let temp = parent.join(format!(".{file_name}.tmp-{}-{unique}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        sync_parent(path)
    };
    write().map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        GenerateError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn sync_parent(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()
}

/// Reads the pointer's generation id; a missing or malformed pointer
/// counts as no current generation (ids still advance via the on-disk
/// immutable-manifest scan, so a corrupt pointer can never recycle ids).
fn pointer_generation(build_dir: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(build_dir.join(MANIFEST_FILE)).ok()?;
    let manifest: PointerManifest = facet_json::from_str(&raw).ok()?;
    let _ = manifest.version;
    Some(manifest.generation_id)
}

/// Every `manifest.<id>.json` id present on disk, committed or stranded.
/// Scan failures propagate — an unreadable build directory must not
/// silently restart id allocation.
fn generation_ids_on_disk(build_dir: &Path) -> Result<Vec<u64>> {
    let entries = match std::fs::read_dir(build_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(GenerateError::Write {
                path: build_dir.to_path_buf(),
                source,
            });
        }
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| GenerateError::Write {
            path: build_dir.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(id) = name
            .strip_prefix("manifest.")
            .and_then(|rest| rest.strip_suffix(".json"))
            .and_then(|id| id.parse().ok())
        {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn read_match_lock(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GenerateError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn newer_match_lock_version(bytes: &[u8]) -> Option<u32> {
    let input = std::str::from_utf8(bytes).ok()?;
    let version: MatchLockVersion = facet_yaml::from_str(input).ok()?;
    (version.version > FILTER_MATCH_LOCK_VERSION).then_some(version.version)
}

fn stale_match_lock_message(
    current: Option<&FilterMatchLock>,
    desired: &FilterMatchLock,
    detail: &str,
) -> String {
    let mut lines = vec![detail.to_string()];
    if let Some(current) = current {
        if current.version != desired.version {
            lines.push(format!("- version {}", current.version));
            lines.push(format!("+ version {}", desired.version));
        }
        let old = current.semantic_lines();
        let new = desired.semantic_lines();
        lines.extend(old.difference(&new).map(|line| format!("- {line}")));
        lines.extend(new.difference(&old).map(|line| format!("+ {line}")));
    } else {
        lines.extend(
            desired
                .semantic_lines()
                .into_iter()
                .map(|line| format!("+ {line}")),
        );
    }
    lines.push("run `dsql lock` to review and accept the current matches".to_string());
    lines.join("\n")
}

fn reconcile_match_lock_inner(
    path: &Path,
    desired: &FilterMatchLock,
    mode: MatchLockMode,
) -> Result<MatchLockStatus> {
    if desired.version != FILTER_MATCH_LOCK_VERSION {
        return Err(GenerateError::Internal(format!(
            "assembled filter match lock has unsupported version {}",
            desired.version
        )));
    }
    let existing = read_match_lock(path)?;
    if let Some(version) = existing.as_deref().and_then(newer_match_lock_version) {
        return Err(GenerateError::MatchLock {
            path: path.to_path_buf(),
            message: format!(
                "version {version} is newer than supported version {FILTER_MATCH_LOCK_VERSION}; refusing to overwrite it"
            ),
        });
    }

    match mode {
        MatchLockMode::Update if desired.is_empty() => {
            let Some(_) = existing else {
                return Ok(MatchLockStatus {
                    changed: false,
                    content_hash: None,
                });
            };
            std::fs::remove_file(path).map_err(|source| GenerateError::Write {
                path: path.to_path_buf(),
                source,
            })?;
            sync_parent(path).map_err(|source| GenerateError::Write {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(MatchLockStatus {
                changed: true,
                content_hash: None,
            })
        }
        MatchLockMode::Update => {
            let serialized = desired
                .to_yaml()
                .map_err(|message| GenerateError::Serialize {
                    name: path.to_string_lossy().to_string(),
                    message,
                })?;
            let changed = existing.as_deref() != Some(serialized.as_bytes());
            if changed {
                atomic_write(path, &serialized)?;
            }
            Ok(MatchLockStatus {
                changed,
                content_hash: Some(sha256_hex(serialized.as_bytes())),
            })
        }
        MatchLockMode::Locked if desired.is_empty() && existing.is_none() => Ok(MatchLockStatus {
            changed: false,
            content_hash: None,
        }),
        MatchLockMode::Locked if desired.is_empty() => Err(GenerateError::MatchLock {
            path: path.to_path_buf(),
            message: stale_match_lock_message(
                None,
                desired,
                "the lock file should be absent because the project has no effective filters",
            ),
        }),
        MatchLockMode::Locked => {
            let Some(existing) = existing else {
                return Err(GenerateError::MatchLock {
                    path: path.to_path_buf(),
                    message: stale_match_lock_message(None, desired, "the lock file is missing"),
                });
            };
            let raw = std::str::from_utf8(&existing).map_err(|error| GenerateError::MatchLock {
                path: path.to_path_buf(),
                message: format!("the lock is not UTF-8: {error}\nrun `dsql lock` to replace it"),
            })?;
            let current =
                FilterMatchLock::from_yaml(raw).map_err(|error| GenerateError::MatchLock {
                    path: path.to_path_buf(),
                    message: format!(
                        "the lock is malformed: {error}\nrun `dsql lock` to replace it"
                    ),
                })?;
            if current != *desired {
                return Err(GenerateError::MatchLock {
                    path: path.to_path_buf(),
                    message: stale_match_lock_message(
                        Some(&current),
                        desired,
                        "the accepted filter matches are stale",
                    ),
                });
            }
            Ok(MatchLockStatus {
                changed: false,
                content_hash: Some(sha256_hex(&existing)),
            })
        }
    }
}

/// Reconciles only `dsql.lock` while holding the same advisory lock used by
/// artifact publication.
pub fn reconcile_match_lock(
    build_dir: &Path,
    path: &Path,
    desired: &FilterMatchLock,
    mode: MatchLockMode,
) -> Result<MatchLockStatus> {
    with_publication_lock(build_dir, || {
        reconcile_match_lock_inner(path, desired, mode)
    })
}

/// Builds the manifest document for `snapshot` at `generation_id`.
fn manifest_for(snapshot: &GenerationSnapshot, generation_id: u64) -> BuildManifest {
    let mut operations = Vec::new();
    let mut fragments = Vec::new();
    for artifact in &snapshot.artifacts {
        let path = artifact_file_name(artifact.family, &artifact.name, &artifact.hash);
        match artifact.family {
            ArtifactFamily::Operation => operations.push(OperationManifestEntry {
                name: artifact.name.clone(),
                kind: artifact.kind,
                path,
                hash: artifact.hash.clone(),
                source: artifact.source.clone(),
            }),
            ArtifactFamily::Fragment => fragments.push(FragmentManifestEntry {
                name: artifact.name.clone(),
                kind: artifact.kind,
                path,
                hash: artifact.hash.clone(),
                source: artifact.source.clone(),
            }),
        }
    }
    BuildManifest {
        version: BUILD_MANIFEST_VERSION,
        generation_id,
        operations,
        fragments,
    }
}

/// Publishes `snapshot`: under the lock, allocates the next generation id
/// (max of the pointer and every immutable manifest on disk, plus one —
/// stranded ids are skipped, never reused), writes content-addressed
/// artifact files (byte-comparing collisions), the immutable manifest,
/// and finally the pointer.
pub fn publish(
    build_dir: &Path,
    match_lock_path: &Path,
    snapshot: &GenerationSnapshot,
    mode: MatchLockMode,
) -> Result<PublishedGeneration> {
    publish_with_deadline(build_dir, match_lock_path, snapshot, mode, LOCK_DEADLINE)
}

/// [`publish`] with an explicit lock-wait bound (tests keep it short).
pub fn publish_with_deadline(
    build_dir: &Path,
    match_lock_path: &Path,
    snapshot: &GenerationSnapshot,
    mode: MatchLockMode,
    wait: Duration,
) -> Result<PublishedGeneration> {
    with_publication_lock_deadline(build_dir, wait, || {
        let match_lock =
            reconcile_match_lock_inner(match_lock_path, &snapshot.filter_match_lock, mode)?;
        let predecessor = pointer_generation(build_dir);
        let generation_id = generation_ids_on_disk(build_dir)?
            .into_iter()
            .chain(predecessor)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| GenerateError::Assembly {
                name: MANIFEST_FILE.to_string(),
                message: "generation counter overflow".to_string(),
            })?;

        let mut written = Vec::new();
        for artifact in &snapshot.artifacts {
            let relative = artifact_file_name(artifact.family, &artifact.name, &artifact.hash);
            let path = build_dir.join(&relative);
            // Only a genuinely absent file is writable; every other read
            // failure (permissions, invalid UTF-8) must not silently turn
            // into a replacement.
            match std::fs::read(&path) {
                Ok(existing) if existing == artifact.serialized.as_bytes() => continue,
                Ok(_) => {
                    return Err(GenerateError::AddressCollision {
                        path: relative,
                        id: artifact.id.clone(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(GenerateError::Write { path, source });
                }
            }
            atomic_write(&path, &artifact.serialized)?;
            written.push(path);
        }

        let manifest = manifest_for(snapshot, generation_id);
        let serialized =
            facet_json::to_string(&manifest).map_err(|error| GenerateError::Serialize {
                name: MANIFEST_FILE.to_string(),
                message: error.to_string(),
            })?;
        let manifest_path = build_dir.join(generation_manifest_file(generation_id));
        // Immutable means immutable: the allocated id cannot exist on
        // disk (allocation maxed over the scan, under the lock), so an
        // existing file here is corruption we refuse to touch.
        if manifest_path.exists() {
            return Err(GenerateError::AddressCollision {
                path: generation_manifest_file(generation_id),
                id: format!("generation {generation_id}"),
            });
        }
        atomic_write(&manifest_path, &serialized)?;
        written.push(manifest_path.clone());
        let current_manifest_path = build_dir.join(MANIFEST_FILE);
        atomic_write(&current_manifest_path, &serialized)?;

        Ok(PublishedGeneration {
            generation_id,
            manifest_path,
            manifest_json: serialized,
            current_manifest_path,
            written,
            filter_match_lock_hash: match_lock.content_hash,
            predecessor,
        })
    })
}

/// Best-effort post-commit maintenance: re-acquires the lock, and — only
/// if the pointer still names `published`'s generation — removes artifact
/// files and immutable manifests referenced by neither this generation
/// nor its recorded predecessor. A superseded publisher skips entirely
/// (the newer one owns maintenance). Failures log to stderr; the commit
/// already happened.
pub fn prune(build_dir: &Path, published: &PublishedGeneration) {
    let outcome = with_publication_lock(build_dir, || {
        if pointer_generation(build_dir) != Some(published.generation_id) {
            return Ok(());
        }
        // An unreadable retained manifest must ABORT pruning: treating
        // it as an empty reference set would delete live artifacts.
        let mut retained: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut retain_manifest = |id: u64| -> Result<()> {
            let path = build_dir.join(generation_manifest_file(id));
            let raw = std::fs::read_to_string(&path).map_err(|source| GenerateError::Write {
                path: path.clone(),
                source,
            })?;
            let manifest: BuildManifest =
                facet_json::from_str(&raw).map_err(|error| GenerateError::Serialize {
                    name: path.to_string_lossy().to_string(),
                    message: error.to_string(),
                })?;
            for entry in &manifest.operations {
                retained.insert(entry.path.clone());
            }
            for entry in &manifest.fragments {
                retained.insert(entry.path.clone());
            }
            Ok(())
        };
        retain_manifest(published.generation_id)?;
        if let Some(predecessor) = published.predecessor {
            retain_manifest(predecessor)?;
        }

        for id in generation_ids_on_disk(build_dir)? {
            if Some(id) != published.predecessor && id != published.generation_id {
                let path = build_dir.join(generation_manifest_file(id));
                if let Err(error) = std::fs::remove_file(&path) {
                    eprintln!("dsql: could not prune {}: {error}", path.display());
                }
            }
        }
        for family in ["operations", "fragments"] {
            let dir = build_dir.join(family);
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    eprintln!(
                        "dsql: could not scan {} for pruning: {error}",
                        dir.display()
                    );
                    continue;
                }
            };
            let entries = entries.filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    eprintln!(
                        "dsql: could not read a pruning entry in {}: {error}",
                        dir.display()
                    );
                    None
                }
            });
            for entry in entries {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with('.') {
                    continue;
                }
                let relative = format!("{family}/{name}");
                if !retained.contains(&relative)
                    && let Err(error) = std::fs::remove_file(entry.path())
                {
                    eprintln!("dsql: could not prune {}: {error}", entry.path().display());
                }
            }
        }
        Ok(())
    });
    if let Err(error) = outcome {
        eprintln!("dsql: build-tree pruning skipped: {error}");
    }
}
