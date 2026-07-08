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

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use bowl::{Bowl, Component, Entity};
use ropey::Rope;

/// Path identity of a source file entity, as given by the loader or editor.
///
/// Fingerprinted so re-inserting the same path invalidates nothing.
#[derive(Component, Hash, PartialEq, Eq, Debug, Clone)]
#[component(hash)]
pub struct FilePath(pub String);

/// Source text of a file entity, rope-backed for cheap incremental edits.
///
/// The fingerprint is the [`SourceText::revision`] verbatim, so
/// fingerprinting never rehashes the rope on an edit burst. Revisions come
/// from a process-global counter: any newly constructed or mutated
/// `SourceText` has a revision strictly greater than every earlier one, so
/// wholesale replacement can never collide with the value it replaces.
#[derive(Component)]
#[component(revision)]
pub struct SourceText {
    rope: Rope,
    revision: u64,
}

/// Process-global revision source backing the [`SourceText`] fingerprint
/// monotonicity guarantee.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(0);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

impl SourceText {
    /// Creates source text from a contiguous string (disk load, `didOpen`).
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            revision: next_revision(),
        }
    }

    /// Replaces the entire text (LSP full-document sync, `didClose` revert).
    pub fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.revision = next_revision();
    }

    /// Applies one incremental edit: replaces `byte_range` with
    /// `replacement`. LSP `didChange` events apply their edits in order
    /// through this, each against the rope the previous edit produced.
    pub fn apply_edit(&mut self, byte_range: std::ops::Range<usize>, replacement: &str) {
        self.rope.remove(byte_range.clone());
        self.rope.insert(byte_range.start, replacement);
        self.revision = next_revision();
    }

    /// The rope, for span slicing and position mapping at protocol
    /// boundaries. Incremental edit helpers land with the LSP crate.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Revision of the current content; bumped on every mutation.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Materializes the full text. Only the parse boundary should need this;
    /// everything else works on byte spans over the parsed snapshot.
    pub fn to_text(&self) -> String {
        self.rope.to_string()
    }
}

/// Marks a file entity whose text is owned by a live editor buffer. Disk
/// watchers and loaders must not overwrite the text while this is present.
#[derive(Component, Hash)]
#[component(hash)]
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
        std::iter::once(scope).chain(
            self.0
                .get(scope)
                .into_iter()
                .flatten()
                .map(String::as_str),
        )
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

/// Inserts in-memory text as a file entity in `scope`.
pub async fn insert_source_scoped(
    bowl: &Bowl,
    path: impl Into<String>,
    text: &str,
    scope: ResolutionScope,
) -> Entity {
    let inserted = bowl
        .insert((FilePath(path.into()), SourceText::from_text(text), scope))
        .await;
    inserted.entity()
}
