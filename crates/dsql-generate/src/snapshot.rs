//! Storage-independent artifact snapshot assembled from settled language facts.

/// Full lowercase-hex SHA-256 — the one protocol hash (artifact hashes,
/// host content hashes); the engine's fast source fingerprint is separate.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The filename address: the hash's first 16 hex characters.
pub fn artifact_address(hash: &str) -> &str {
    &hash[..16.min(hash.len())]
}

pub(crate) const OPERATIONS_DIR: &str = "operations";
pub(crate) const FRAGMENTS_DIR: &str = "fragments";

/// The case-folded path two artifacts of one family may not share.
pub(crate) fn artifact_collision_key(family: ArtifactFamily, name: &str) -> String {
    format!(
        "{}/{}",
        artifact_directory(family),
        artifact_file_stem(name).to_ascii_lowercase()
    )
}

pub(crate) fn artifact_directory(family: ArtifactFamily) -> &'static str {
    match family {
        ArtifactFamily::Operation => OPERATIONS_DIR,
        ArtifactFamily::Fragment => FRAGMENTS_DIR,
    }
}

pub(crate) fn artifact_file_stem(name: &str) -> String {
    let mut output = String::new();
    for char in name.chars() {
        if char.is_ascii_alphanumeric() || char == '-' || char == '_' {
            output.push(char);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "operation".to_string()
    } else {
        output
    }
}

/// One assembled artifact ready for a storage adapter.
#[derive(Debug, Clone)]
pub struct SnapshotArtifact {
    /// Stable opaque id: `scope/kind/name`. Consumers key on this.
    pub id: String,
    /// `operation` | `fragment` — the artifact family.
    pub family: ArtifactFamily,
    /// The metadata's own kind string (`query`, `fragment`).
    pub kind: String,
    pub scope: String,
    pub name: String,
    /// Serialized metadata JSON, exactly as a native publisher writes it.
    pub serialized: String,
    /// Full SHA-256 of `serialized`, lowercase hex.
    pub hash: String,
    /// Logical source path.
    pub source: String,
}

/// Physical artifact namespace used by publication and manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactFamily {
    Operation,
    Fragment,
}

impl ArtifactFamily {
    /// Stable directory/protocol label for the family.
    pub fn label(self) -> &'static str {
        match self {
            Self::Operation => "operation",
            Self::Fragment => "fragment",
        }
    }
}

/// One resolution scope's effective artifact view.
#[derive(Debug, Clone)]
pub struct SnapshotGroup {
    pub name: String,
    pub imports: Vec<String>,
    pub artifacts: Vec<String>,
}

/// The fully assembled build tree before any storage adapter writes it.
#[derive(Debug, Clone)]
pub struct GenerationSnapshot {
    /// Sorted by id.
    pub artifacts: Vec<SnapshotArtifact>,
    /// Sorted by name.
    pub groups: Vec<SnapshotGroup>,
    /// Canonical filter match decisions paired with these artifacts.
    pub filter_match_lock: crate::FilterMatchLock,
}
