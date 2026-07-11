//! The source model: how text enters the bowl.
//!
//! There is one text component, [`SourceText`], and two writers:
//!
//! - **Analysis** (CLI, generate, tests) reads a file from disk once via
//!   [`load_file`] and never touches it again.
//! - **LSP** additionally marks the entity with [`OpenBuffer`] and applies
//!   incremental rope edits through porridge's external mutation
//!   (`Mut<SourceText>`), bumping the revision per edit.
//!
//! Downstream systems (parse, lowering, checks, services) cannot tell the
//! two apart. Spans are byte ranges end to end; materialization to a
//! contiguous `String` happens only at the parse boundary.
//!
//! Not every file entity is a dsql document. TypeScript sources carry
//! [`EmbeddingHost`]; the extraction system (`embedding`) derives one
//! *region* entity per embedded query, and those regions — not the host —
//! are the dsql documents. [`DsqlDocument`] marks what the parse consumes:
//! plain `.dsql` files get it from the insert helpers, regions from
//! extraction. Both writers above write *host* text the same way they
//! write plain files; regions re-derive from it either way.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use bowl::{Bowl, Component, Entity};
use ropey::Rope;

/// Path identity of a source file entity, as given by the loader or editor.
///
/// Fingerprinted so re-inserting the same path invalidates nothing.
#[derive(Component, Hash, PartialEq, Eq, Debug, Clone)]
#[component(hash)]
pub struct FilePath(pub String);

/// Byte offset of a document's text inside its host file: zero for plain
/// `.dsql` files, the embedded region's start for documents extracted from
/// another language's source. Spans stay document-relative everywhere;
/// this shifts them back to host coordinates at reporting boundaries.
#[derive(Component, Hash, PartialEq, Eq, Debug, Clone, Copy)]
#[component(hash)]
pub struct SourceOffset(pub usize);

/// Marks a file entity whose [`SourceText`] is a dsql document — the
/// parse's input filter. Host sources carry text too, but in another
/// language; only their extracted regions are documents.
#[derive(Component, Hash)]
#[component(hash)]
pub struct DsqlDocument;

/// Marks a file entity as a host source: text in another language with
/// dsql documents embedded in it. The extraction system derives one
/// region entity per embedded document.
#[derive(Component, Hash)]
#[component(hash)]
pub struct EmbeddingHost;

/// Join key from an extracted region back to its host file entity, the
/// counterpart of [`crate::facts::BelongsToFile`] one level up.
#[derive(Component, Hash, PartialEq, Eq, Debug, Clone, Copy)]
#[component(hash)]
pub struct BelongsToHost(pub Entity);

/// Source text of a file entity, rope-backed for cheap incremental edits.
///
/// The fingerprint is a hash of the rope's content, so equal text always
/// means an equal fingerprint no matter who produced it: an edit burst
/// that lands back on the previous text, a disk reload of unchanged
/// content, or a derivation (embedded-region extraction) re-emitting an
/// untouched region all leave downstream facts valid.
#[derive(Component, Hash)]
#[component(hash)]
pub struct SourceText {
    rope: Rope,
}

impl SourceText {
    /// Creates source text from a contiguous string (disk load, `didOpen`,
    /// region extraction).
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    /// Replaces the entire text (LSP full-document sync, `didClose` revert).
    pub fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
    }

    /// Applies one incremental edit: replaces `byte_range` with
    /// `replacement`. LSP `didChange` events apply their edits in order
    /// through this, each against the rope the previous edit produced.
    pub fn apply_edit(&mut self, byte_range: std::ops::Range<usize>, replacement: &str) {
        self.rope.remove(byte_range.clone());
        self.rope.insert(byte_range.start, replacement);
    }

    /// The rope, for span slicing and position mapping at protocol
    /// boundaries. Incremental edit helpers land with the LSP crate.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Materializes the full text. Only the parse boundary should need this;
    /// everything else works on byte spans over the parsed snapshot.
    pub fn to_text(&self) -> String {
        self.rope.to_string()
    }
}

/// Marks a file entity whose text is owned by a live editor buffer. Disk
/// watchers and loaders must not overwrite the text while this is present.
///
/// Untracked: this is bookkeeping for text *writers*, not an input any
/// derivation reads. A tracked marker would lift the file's entity
/// revision when the editor stamps it, retiring every `DerivedFrom`-
/// anchored fact of the file without anything re-deriving them (the
/// text's own fingerprint is unchanged) — externally inserted markers on
/// fact-bearing entities must stay out of change tracking.
#[derive(Component, Hash)]
#[component(hash, untracked)]
pub struct OpenBuffer;

/// The resolution scope a file belongs to (docs/spec/resolution-scopes.md).
/// Queries and fragments resolve against the definitions visible to their
/// file's scope: the scope's own plus its imports'. Projects without scope
/// configuration put every file in [`ResolutionScope::DEFAULT`].
#[derive(Component, Hash, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct ResolutionScope(pub String);

impl ResolutionScope {
    pub const DEFAULT: &'static str = "default";

    pub fn default_scope() -> Self {
        Self(Self::DEFAULT.to_string())
    }
}

/// Region-to-host projection: where a document's facts render or
/// publish. Regions project onto their host at their extraction offset;
/// plain files project onto themselves at offset zero. One scoop of the
/// region rows feeds every consumer (CLI rendering, LSP publishing,
/// token aggregation), so the projection rule exists once.
pub struct HostProjection(std::collections::HashMap<Entity, (Entity, usize)>);

impl HostProjection {
    /// Builds the projection from `(region, host, offset)` rows.
    pub fn new(regions: impl IntoIterator<Item = (Entity, Entity, usize)>) -> Self {
        Self(
            regions
                .into_iter()
                .map(|(region, host, offset)| (region, (host, offset)))
                .collect(),
        )
    }

    /// The file a document's spans render against, with the byte offset
    /// to add.
    pub fn target_of(&self, file: Entity) -> (Entity, usize) {
        self.0.get(&file).copied().unwrap_or((file, 0))
    }

    /// The inverse: every document projecting onto `host`, with its
    /// offset, in ascending offset order.
    pub fn documents_of(&self, host: Entity) -> Vec<(Entity, usize)> {
        let mut documents: Vec<(Entity, usize)> = self
            .0
            .iter()
            .filter(|(_, (target, _))| *target == host)
            .map(|(region, (_, offset))| (*region, *offset))
            .collect();
        documents.sort_by_key(|(_, offset)| *offset);
        documents
    }
}

/// The configured document paths per resolution scope (absolute, already
/// joined against the project base): the bowl-carried source of scope
/// *placement*, so an editor opening a brand-new file can ask which scope
/// owns its path without any adapter-side project state. Empty without
/// project configuration — everything lands in the default scope.
#[derive(Component, Debug, Default, Hash)]
#[component(hash)]
pub struct ScopeDocuments(pub Vec<(String, Vec<String>)>);

/// Which resolution scope owns a path under the configured patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeOwnership {
    /// No scopes are configured: the project's implicit default scope.
    ImplicitDefault,
    /// Exactly one configured scope covers the path.
    Unique(String),
    /// A configured project, but no pattern covers the path: the file is
    /// outside the project's document set (standalone editing).
    Unmatched,
    /// More than one scope covers the path — an ownership error at load;
    /// callers pick a deterministic policy and say so.
    Ambiguous(Vec<String>),
}

impl ScopeDocuments {
    /// Resolves which configured scope owns `path`, preserving the
    /// unmatched/ambiguous outcomes so callers can react instead of
    /// silently defaulting.
    pub fn ownership_of(&self, path: &str) -> ScopeOwnership {
        if self.0.is_empty() {
            return ScopeOwnership::ImplicitDefault;
        }
        let candidate = Path::new(path);
        let mut matches: Vec<String> = Vec::new();
        for (scope, configured) in &self.0 {
            if configured
                .iter()
                .any(|entry| configured_path_matches(Path::new(entry), candidate))
            {
                matches.push(scope.clone());
            }
        }
        matches.sort_unstable();
        matches.dedup();
        match matches.len() {
            0 => ScopeOwnership::Unmatched,
            1 => ScopeOwnership::Unique(matches.remove(0)),
            _ => ScopeOwnership::Ambiguous(matches),
        }
    }
}

/// Whether one configured document path — glob, directory, or plain file —
/// covers `path`, without touching the filesystem.
fn configured_path_matches(configured: &Path, path: &Path) -> bool {
    let raw = configured.to_string_lossy();
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        return glob::Pattern::new(&raw)
            .ok()
            .is_some_and(|pattern| pattern.matches_path(path));
    }
    if path == configured {
        return is_document_path(path);
    }
    path.starts_with(configured) && is_document_path(path)
}

/// Whether the path carries a dsql document or host extension.
fn is_document_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "dsql" | "ts" | "tsx"))
}

/// The scope import graph as a fingerprinted singleton fact: scope name →
/// directly imported scope names. Imports are not transitive: an importing
/// scope sees the imported scopes' own definitions only. A default (empty)
/// value is installed at language registration; project loading replaces it.
#[derive(Component, Hash, Debug, Clone, Default, PartialEq, Eq)]
#[component(hash)]
pub struct ScopeImports(pub BTreeMap<String, Vec<String>>);

impl ScopeImports {
    /// The scopes visible from `scope`: itself plus its direct imports.
    pub fn visible_from<'a>(&'a self, scope: &'a str) -> impl Iterator<Item = &'a str> {
        std::iter::once(scope).chain(self.0.get(scope).into_iter().flatten().map(String::as_str))
    }

    /// The imports of `scope`, without the scope itself.
    pub fn imports_of<'a>(&'a self, scope: &'a str) -> impl Iterator<Item = &'a str> {
        self.0.get(scope).into_iter().flatten().map(String::as_str)
    }
}

/// Analysis-path loader: reads `path` from disk and inserts it as a file
/// entity in the default scope. The returned [`Entity`] identifies the
/// file for later scoops.
pub async fn load_file(bowl: &Bowl, path: impl AsRef<Path>) -> io::Result<Entity> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)?;
    Ok(insert_source(bowl, path.display().to_string(), &text).await)
}

/// Inserts in-memory text as a file entity in the default scope.
pub async fn insert_source(bowl: &Bowl, path: impl Into<String>, text: &str) -> Entity {
    insert_source_scoped(bowl, path, text, ResolutionScope::default_scope()).await
}

/// Extensions whose files are host sources rather than dsql documents —
/// the one classifier every layer shares (loading, formatting, editors).
pub fn is_host_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "ts" | "tsx"))
}

/// Inserts in-memory text as a file entity in `scope`. The one entry point
/// for every writer: the path's extension decides whether the text is a
/// dsql document or a host source whose regions extraction derives.
pub async fn insert_source_scoped(
    bowl: &Bowl,
    path: impl Into<String>,
    text: &str,
    scope: ResolutionScope,
) -> Entity {
    let path = path.into();
    let inserted = if is_host_path(&path) {
        bowl.insert((
            FilePath(path),
            EmbeddingHost,
            SourceText::from_text(text),
            scope,
        ))
        .await
    } else {
        bowl.insert((
            FilePath(path),
            SourceOffset(0),
            DsqlDocument,
            SourceText::from_text(text),
            scope,
        ))
        .await
    };
    inserted.entity()
}
