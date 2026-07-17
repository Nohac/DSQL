//! The source model: how text enters the bowl.
//!
//! There is one text component, [`SourceText`], and two writers:
//!
//! - **Analysis** (CLI, generate, tests, browser tools) supplies complete
//!   in-memory text through [`insert_source_scoped`] and never touches it
//!   again.
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

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
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

/// How project configuration resolves one physical source into dsql documents.
/// Paths select files; this value selects full-document parsing or a named
/// embedded-document extractor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceKind {
    /// The whole physical source is one dsql document.
    Dsql,
    /// The physical source is a host interpreted by the named extractor.
    Embedded(String),
}

impl SourceKind {
    /// The built-in resolver name for full-document dsql sources.
    pub const DSQL_RESOLVER: &'static str = "dsql";

    /// Converts a configured resolver name into its runtime source kind.
    pub fn from_resolver(resolver: impl Into<String>) -> Self {
        let resolver = resolver.into();
        if resolver == Self::DSQL_RESOLVER {
            Self::Dsql
        } else {
            Self::Embedded(resolver)
        }
    }

    /// The configured resolver name represented by this source kind.
    pub fn resolver(&self) -> &str {
        match self {
            Self::Dsql => Self::DSQL_RESOLVER,
            Self::Embedded(resolver) => resolver,
        }
    }
}

/// The named extractor selected for an [`EmbeddingHost`].
#[derive(Component, Hash, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct ExtractionResolver(pub String);

/// Join key from an extracted region back to its host file entity, the
/// counterpart of [`crate::facts::BelongsToFile`] one level up.
#[derive(Component, Hash, PartialEq, Eq, Debug, Clone, Copy)]
#[component(hash)]
pub struct BelongsToHost(pub Entity);

/// Content fingerprint for contiguous source text.
pub fn content_hash(text: &str) -> u64 {
    fingerprint(text)
}

fn fingerprint<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// An edit was applied to source text whose rope has been evicted.
/// Rehydrate with [`SourceText::set_text`] first — incremental edits have
/// nothing to edit against.
#[derive(Debug, thiserror::Error)]
#[error("source text is not resident; rehydrate with set_text before editing")]
pub struct SourceTextNotResident;

/// Source text of a file entity, rope-backed for cheap incremental edits.
///
/// The engine fingerprint is the stored `hash` of the rope's *content*
/// (never the rope itself), so equal text always means an equal
/// fingerprint no matter who produced it: an edit burst that lands back
/// on the previous text, a disk reload of unchanged content, or a
/// derivation (embedded-region extraction) re-emitting an untouched
/// region all leave downstream facts valid.
///
/// The rope is *evictable* (docs/issues.md, hash-carrying evictable
/// ropes): batch analysis drops it after the parse boundary has
/// materialized what derivations need, while the stored hash keeps the
/// component's fingerprint — and with it every derived fact — intact.
/// [`SourceText::set_text`] rehydrates (a disk reload of unchanged
/// content is hash-neutral; changed content bumps, correctly).
#[derive(Component)]
#[component(hash)]
pub struct SourceText {
    rope: Option<Rope>,
    hash: u64,
}

impl Hash for SourceText {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // The stored content hash only: residency is not identity.
        state.write_u64(self.hash);
    }
}

impl SourceText {
    /// Creates source text from a contiguous string (disk load, `didOpen`,
    /// region extraction).
    pub fn from_text(text: &str) -> Self {
        let rope = Rope::from_str(text);
        let hash = fingerprint(&rope);
        Self {
            rope: Some(rope),
            hash,
        }
    }

    /// Replaces the entire text (LSP full-document sync, `didClose`
    /// revert, rehydration of an evicted rope).
    pub fn set_text(&mut self, text: &str) {
        *self = Self::from_text(text);
    }

    /// Applies one incremental edit: replaces `byte_range` with
    /// `replacement`. LSP `didChange` events apply their edits in order
    /// through this, each against the rope the previous edit produced.
    /// Fails on an evicted rope — editor-owned buffers are never evicted,
    /// so a failure here is a residency-policy bug, not an editing state.
    pub fn apply_edit(
        &mut self,
        byte_range: std::ops::Range<usize>,
        replacement: &str,
    ) -> Result<(), SourceTextNotResident> {
        let Some(rope) = self.rope.as_mut() else {
            return Err(SourceTextNotResident);
        };
        rope.remove(byte_range.clone());
        rope.insert(byte_range.start, replacement);
        self.hash = fingerprint(rope);
        Ok(())
    }

    /// The rope, for span slicing and position mapping at protocol
    /// boundaries; `None` once evicted.
    pub fn rope(&self) -> Option<&Rope> {
        self.rope.as_ref()
    }

    /// Materializes the full text; `None` once evicted. Only the parse
    /// boundary should need this; everything else works on byte spans
    /// over the parsed snapshot.
    pub fn to_text(&self) -> Option<String> {
        self.rope.as_ref().map(Rope::to_string)
    }

    /// Drops the rope, keeping the content hash — fingerprint-neutral, so
    /// nothing derived from this text retires or re-derives.
    pub fn evict(&mut self) {
        self.rope = None;
    }

    /// Whether the rope is currently materialized (not evicted).
    pub fn is_resident(&self) -> bool {
        self.rope.is_some()
    }

    /// The stored content fingerprint (what the engine hashes).
    pub fn content_hash(&self) -> u64 {
        self.hash
    }
}

/// Arms batch-analysis residency: source ropes are evicted at the parse
/// and extraction boundaries once derivations have what they need.
/// Editor-facing bowls must never call this.
pub async fn arm_analysis_residency(bowl: &bowl::Bowl) {
    bowl.insert((
        bowl::Singleton::<AnalysisResidency>::new(),
        AnalysisResidency,
    ))
    .await;
}

/// Batch-analysis residency: when this singleton is present, the parse
/// and extraction boundaries evict source ropes after materializing what
/// derivations need. Editor-facing bowls never insert it, and entities
/// marked [`OpenBuffer`] are exempt regardless.
#[derive(Component, Hash)]
#[component(hash)]
pub struct AnalysisResidency;

/// The span of the entire embedding callsite expression in the host —
/// the whole `dsql(`…`)`, not just the content between the backticks
/// (docs/spec/build-daemon.md, Callsites): the range a build-tool
/// binding replaces with the generated operation reference.
#[derive(Component, Hash, Debug, Clone, Copy, PartialEq, Eq)]
#[component(hash)]
pub struct CallsiteSpan(pub crate::facts::Span);

/// The span of the embedded document's content in the host — exactly the
/// bytes between the backticks (the extraction pattern's `content`
/// capture). Recorded at extraction like [`CallsiteSpan`] because the
/// region rope may be evicted before assembly reads it; renderers slice
/// this range from the host to key source-string type registries without
/// re-detecting anything (docs/spec/build-daemon.md, Callsites).
#[derive(Component, Hash, Debug, Clone, Copy, PartialEq, Eq)]
#[component(hash)]
pub struct ContentSpan(pub crate::facts::Span);

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
/// owns its path without any adapter-side project state. An empty value is
/// the projectless single-file fallback, where editor-declared dsql documents
/// land in the default scope.
#[derive(Component, Debug, Default, Hash)]
#[component(hash)]
pub struct ScopeDocuments(pub Vec<(String, Vec<ScopeDocument>)>);

/// One configured group of physical paths interpreted by the same resolver.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeDocument {
    pub kind: SourceKind,
    pub paths: Vec<String>,
}

/// A configured source assignment returned to live adapters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceAssignment {
    pub scope: String,
    pub kind: SourceKind,
}

/// Which resolution scope owns a path under the configured patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeOwnership {
    /// No project document assignments are installed: single-file mode.
    ImplicitDefault,
    /// Exactly one configured scope covers the path.
    Unique(SourceAssignment),
    /// A configured project, but no pattern covers the path: the file is
    /// outside the project's document set.
    Unmatched,
    /// More than one scope covers the path — an ownership error at load;
    /// callers pick a deterministic policy and say so.
    Ambiguous(Vec<SourceAssignment>),
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
        let mut matches: Vec<SourceAssignment> = Vec::new();
        for (scope, documents) in &self.0 {
            for document in documents {
                if document
                    .paths
                    .iter()
                    .any(|entry| configured_path_matches(Path::new(entry), candidate))
                {
                    matches.push(SourceAssignment {
                        scope: scope.clone(),
                        kind: document.kind.clone(),
                    });
                }
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
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..glob::MatchOptions::new()
        };
        return glob::Pattern::new(&raw)
            .ok()
            .is_some_and(|pattern| pattern.matches_path_with(path, options));
    }
    if path == configured {
        return true;
    }
    path.starts_with(configured)
}

/// The scope import graph as a fingerprinted singleton fact: scope name →
/// directly imported scope names. Consumers resolve the graph transitively
/// through [`ScopeImports::visible_from`]. A default (empty) value is
/// installed at language registration; project loading replaces it.
#[derive(Component, Hash, Debug, Clone, Default, PartialEq, Eq)]
#[component(hash)]
pub struct ScopeImports(pub BTreeMap<String, Vec<String>>);

impl ScopeImports {
    /// The effective scopes visible from `scope`: itself first, then its
    /// recursive imports in declaration order, with diamond paths deduplicated.
    /// Cycles are cut defensively; project loading rejects them separately.
    pub fn visible_from<'a>(&'a self, scope: &'a str) -> impl Iterator<Item = &'a str> {
        fn collect<'a>(
            graph: &'a ScopeImports,
            scope: &'a str,
            seen: &mut BTreeSet<&'a str>,
            visible: &mut Vec<&'a str>,
        ) {
            if !seen.insert(scope) {
                return;
            }
            visible.push(scope);
            if let Some(imports) = graph.0.get(scope) {
                for import in imports {
                    collect(graph, import, seen, visible);
                }
            }
        }

        let mut seen = BTreeSet::new();
        let mut visible = Vec::new();
        collect(self, scope, &mut seen, &mut visible);
        visible.into_iter()
    }

    /// The effective recursive imports of `scope`, without the scope itself.
    pub fn imports_of<'a>(&'a self, scope: &'a str) -> impl Iterator<Item = &'a str> {
        self.visible_from(scope).skip(1)
    }

    /// Returns the first deterministic import cycle, if the graph is cyclic.
    /// Scope roots are visited lexicographically and edges in declaration
    /// order, matching project diagnostics and snapshot ordering.
    pub fn import_cycle(&self) -> Option<Vec<String>> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum VisitState {
            Visiting,
            Visited,
        }

        fn visit(
            graph: &ScopeImports,
            scope: &str,
            states: &mut BTreeMap<String, VisitState>,
            stack: &mut Vec<String>,
        ) -> Option<Vec<String>> {
            match states.get(scope) {
                Some(VisitState::Visited) => return None,
                Some(VisitState::Visiting) => {
                    let start = stack.iter().position(|entry| entry == scope)?;
                    let mut cycle = stack[start..].to_vec();
                    cycle.push(scope.to_string());
                    return Some(cycle);
                }
                None => {}
            }

            states.insert(scope.to_string(), VisitState::Visiting);
            stack.push(scope.to_string());
            if let Some(imports) = graph.0.get(scope) {
                for import in imports {
                    if let Some(cycle) = visit(graph, import, states, stack) {
                        return Some(cycle);
                    }
                }
            }
            stack.pop();
            states.insert(scope.to_string(), VisitState::Visited);
            None
        }

        let mut states = BTreeMap::new();
        let mut stack = Vec::new();
        for scope in self.0.keys() {
            if let Some(cycle) = visit(self, scope, &mut states, &mut stack) {
                return Some(cycle);
            }
        }
        None
    }
}

/// Inserts in-memory text as a file entity in the default scope.
pub async fn insert_source(bowl: &Bowl, path: impl Into<String>, text: &str) -> Entity {
    insert_source_scoped(
        bowl,
        path,
        text,
        ResolutionScope::default_scope(),
        SourceKind::Dsql,
    )
    .await
}

/// Inserts an embedding host in the default scope with an explicit extractor.
pub async fn insert_embedding_source(
    bowl: &Bowl,
    path: impl Into<String>,
    text: &str,
    resolver: impl Into<String>,
) -> Entity {
    insert_source_scoped(
        bowl,
        path,
        text,
        ResolutionScope::default_scope(),
        SourceKind::Embedded(resolver.into()),
    )
    .await
}

/// Inserts in-memory text as a file entity in `scope`, interpreted by the
/// resolver selected in project or editor configuration.
pub async fn insert_source_scoped(
    bowl: &Bowl,
    path: impl Into<String>,
    text: &str,
    scope: ResolutionScope,
    kind: SourceKind,
) -> Entity {
    let path = path.into();
    let inserted = match kind {
        SourceKind::Embedded(resolver) => {
            bowl.insert((
                FilePath(path),
                EmbeddingHost,
                ExtractionResolver(resolver),
                SourceText::from_text(text),
                scope,
            ))
            .await
        }
        SourceKind::Dsql => {
            bowl.insert((
                FilePath(path),
                SourceOffset(0),
                DsqlDocument,
                SourceText::from_text(text),
                scope,
            ))
            .await
        }
    };
    inserted.entity()
}
