//! Embedded-document extraction as a derivation.
//!
//! Host sources ([`EmbeddingHost`]) carry another language's text with
//! dsql documents embedded in it. This system derives one *region* entity
//! per embedded document — [`DsqlDocument`] plus its own [`SourceText`] —
//! so everything downstream consumes regions exactly like plain files,
//! and both writers of host text (disk loader, LSP rope edits) get
//! extraction for free.
//!
//! Incrementality comes from two engine properties: derived spawn slots
//! reuse the previous run's entity ids in spawn order, so a region keeps
//! its identity across host edits by ordinal; and [`SourceText`]
//! fingerprints are content hashes, so a re-derived region whose text did
//! not change re-inserts with an equal fingerprint and invalidates
//! nothing. Regions that vanish are reaped by the stale-derived cleanup.

use crate::schema::dsql_schema;
use std::sync::Mutex;

use bowl::{Commands, Component, DerivedFrom, Entity, MutRef, Query, Registrar, View, With};
use regex::Regex;

use crate::source::{
    AnalysisResidency, BelongsToHost, DsqlDocument, EmbeddingHost, OpenBuffer, ResolutionScope,
    SourceOffset, SourceText,
};

/// The extraction pattern, one fingerprinted singleton per bowl: a regex
/// whose named `content` capture is the embedded document. Language
/// registration installs the default; project loading replaces it with
/// the configured pattern after validating it.
#[derive(Component, Hash, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct EmbeddedPattern(pub String);

impl Default for EmbeddedPattern {
    fn default() -> Self {
        Self(default_typescript_pattern().to_string())
    }
}

/// The default TypeScript pattern: `dsql`-tagged template literals, with
/// or without a wrapping call.
pub fn default_typescript_pattern() -> &'static str {
    r"dsql(?:\s*\(\s*)?`(?P<content>[\s\S]*?)`(?:\s*\))?"
}

/// Compiles an extraction pattern, validating the `content` capture.
/// Loaders call this before installing a configured pattern, so an
/// invalid one fails at the configuration boundary, not inside the bowl.
pub fn compile_embedding_pattern(pattern: &str) -> Result<Regex, String> {
    let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
    if regex.capture_names().all(|name| name != Some("content")) {
        return Err("pattern must define a named `content` capture".to_string());
    }
    Ok(regex)
}

/// Installs the extraction system; [`crate::language_bowl`] and project
/// loading install the pattern singleton.
pub(crate) fn register_embedding(reg: &mut Registrar<'_>) {
    reg.system(extract_embedded_documents);
}

/// A host's text with what extraction and the eviction decision need.
type EvictableHost<'a> = Query<
    (
        Entity,
        MutRef<'a, SourceText>,
        &'a ResolutionScope,
        Option<&'a OpenBuffer>,
    ),
    With<EmbeddingHost>,
>;

/// Derives the region entities of one host source: per match, a dsql
/// document carrying the region text, its byte offset in the host, and
/// the host's resolution scope.
async fn extract_embedded_documents(
    hosts: EvictableHost<'_>,
    pattern: Query<(Entity, &EmbeddedPattern)>,
    residency: View<'_, (Entity, &AnalysisResidency)>,
    mut commands: Commands<(dsql_schema::Region,)>,
) {
    let (host, mut text, scope, open_buffer) = hosts.item();
    let (pattern_entity, pattern) = pattern.item();

    // Loaders validate configured patterns up front; the default is
    // statically valid. A bad pattern reaching this point derives nothing.
    let Some(regex) = cached_embedding_pattern(&pattern.0) else {
        return;
    };

    // An evicted rope means this host (same fingerprint) already
    // extracted its regions.
    let Some(source) = text.to_text() else {
        return;
    };
    for captures in regex.captures_iter(&source) {
        let Some(content) = captures.name("content") else {
            continue;
        };
        commands.insert((
            DerivedFrom::many([host, pattern_entity]),
            BelongsToHost(host),
            DsqlDocument,
            SourceOffset(content.start()),
            scope.clone(),
            SourceText::from_text(content.as_str()),
        ));
    }
    // Hosts are the largest files and never receive a ParsedFile; the
    // extraction boundary is their consumption point.
    if residency.iter().next().is_some() && open_buffer.is_none() {
        text.evict();
    }
}

/// Compiled-pattern cache: extraction reruns per keystroke on open host
/// buffers, and NFA construction is far too expensive for that. One slot
/// suffices — a bowl has one pattern, and pattern changes are config
/// events.
static COMPILED_PATTERN: Mutex<Option<(String, Regex)>> = Mutex::new(None);

fn cached_embedding_pattern(pattern: &str) -> Option<Regex> {
    let mut cached = COMPILED_PATTERN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((key, regex)) = cached.as_ref()
        && key == pattern
    {
        return Some(regex.clone());
    }
    let regex = compile_embedding_pattern(pattern).ok()?;
    *cached = Some((pattern.to_string(), regex.clone()));
    Some(regex)
}
