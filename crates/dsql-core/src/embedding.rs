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

use std::collections::BTreeMap;
use std::sync::Mutex;

use bowl::{
    Commands, Component, DerivedFrom, Entity, MutRef, Query, Registrar, SystemExt, View, With,
};
use regex::Regex;

use crate::schema::dsql_schema;
use crate::source::{
    AnalysisResidency, BelongsToHost, CallsiteSpan, ContentSpan, DsqlDocument, EmbeddingHost,
    ExtractionResolver, OpenBuffer, ResolutionScope, SourceOffset, SourceText,
};

/// One embedded-document extraction provider. Regex is implemented now;
/// provider dispatch is explicit so tree-sitter can be added without changing
/// source discovery or host assignment.
#[derive(Hash, Debug, Clone, PartialEq, Eq)]
pub enum ExtractionStrategy {
    /// A regex whose named `content` capture is the embedded document.
    Regex { pattern: String },
}

/// Named extraction providers installed as one fingerprinted bowl input.
#[derive(Component, Hash, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct ExtractionRegistry(pub BTreeMap<String, ExtractionStrategy>);

impl Default for ExtractionRegistry {
    fn default() -> Self {
        Self(BTreeMap::from([(
            "typescript".to_string(),
            ExtractionStrategy::Regex {
                pattern: default_typescript_pattern().to_string(),
            },
        )]))
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
/// loading install the provider registry singleton.
pub(crate) fn register_embedding(reg: &mut Registrar<'_>) {
    reg.system(extract_embedded_documents);
    reg.system(check_embedded_expressions.run_during(bowl::Phase::Complete));
}

/// The rewrite contract's language rules (docs/spec/build-daemon.md,
/// Callsites): an embedded expression must define exactly one query —
/// more are an ambiguous rewrite target, and fragment-only expressions
/// would leave a raw `dsql(…)` value in shipped code (fragment-only
/// *documents* remain fully supported in plain `.dsql` files).
async fn check_embedded_expressions(
    _: Query<Entity, With<crate::facts::DiagnosticsDemand>>,
    regions: Query<(Entity, &BelongsToHost, &CallsiteSpan), With<DsqlDocument>>,
    // The definition index is the tracked input that re-runs this check
    // when definitions appear, change kind, or vanish; the ambient view
    // alone would go stale after the first settle.
    _index: Query<(Entity, &crate::entities::definition::DefIndex)>,
    defs: View<
        '_,
        (
            Entity,
            &crate::entities::definition::DefDecl,
            &crate::facts::BelongsToFile,
        ),
    >,
    mut commands: Commands<(crate::schema::dsql_schema::Diagnostic,)>,
) {
    use crate::entities::definition::DefKind;
    use crate::facts::{
        DiagnosticCode, DiagnosticFacts, DiagnosticSource, Severity, emit_diagnostic,
    };

    // The CallsiteSpan join is the gate: plain documents have none.
    let (region, _, _callsite) = regions.item();
    let queries = defs
        .iter()
        .filter(|(_, decl, file)| file.0 == region && decl.kind == DefKind::Query)
        .count();
    let fragments = defs
        .iter()
        .filter(|(_, decl, file)| file.0 == region && decl.kind == DefKind::Fragment)
        .count();

    // Diagnostics on regions project onto the host; the callsite span is
    // already in host coordinates, so anchor at the region's own origin
    // (span relative to the region = callsite minus the region offset is
    // not needed — report at the region start, offset 0..0 projected).
    let message = if queries > 1 {
        Some(format!(
            "embedded dsql expression defines {queries} queries; a callsite rewrites to exactly one"
        ))
    } else if queries == 0 && fragments > 0 {
        Some(
            "embedded dsql expression defines only fragments; move shared fragments into a \
             .dsql document"
                .to_string(),
        )
    } else if queries == 0 {
        // Empty expressions have no rewrite target either.
        Some(
            "embedded dsql expression defines no query; a callsite rewrites to exactly one"
                .to_string(),
        )
    } else {
        None
    };
    if let Some(message) = message {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(region),
                file: region,
                span: crate::facts::Span { start: 0, end: 0 },
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::EmbeddedExpressionShape,
                message,
            },
        );
    }
}

/// A host's text with what extraction and the eviction decision need.
type EvictableHost<'a> = Query<
    (
        Entity,
        MutRef<'a, SourceText>,
        &'a ResolutionScope,
        &'a ExtractionResolver,
        Option<&'a OpenBuffer>,
    ),
    With<EmbeddingHost>,
>;

/// Derives the region entities of one host source: per match, a dsql
/// document carrying the region text, its byte offset in the host, and
/// the host's resolution scope.
async fn extract_embedded_documents(
    hosts: EvictableHost<'_>,
    registry: Query<(Entity, &ExtractionRegistry)>,
    residency: View<'_, (Entity, &AnalysisResidency)>,
    mut commands: Commands<(dsql_schema::Region,)>,
) {
    let (host, mut text, scope, resolver, open_buffer) = hosts.item();
    let (registry_entity, registry) = registry.item();

    let Some(strategy) = registry.0.get(&resolver.0) else {
        return;
    };
    let regex = match strategy {
        ExtractionStrategy::Regex { pattern } => cached_embedding_pattern(pattern),
    };
    // Loaders validate configured providers up front; a bad pattern reaching
    // this point derives nothing.
    let Some(regex) = regex else {
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
        // The full match is the entire callsite expression — the range a
        // build-tool binding replaces (docs/spec/build-daemon.md).
        let expression = captures.get(0).map_or(
            crate::facts::Span {
                start: content.start(),
                end: content.end(),
            },
            |full| crate::facts::Span {
                start: full.start(),
                end: full.end(),
            },
        );
        commands.insert((
            DerivedFrom::many([host, registry_entity]),
            BelongsToHost(host),
            DsqlDocument,
            SourceOffset(content.start()),
            CallsiteSpan(expression),
            ContentSpan(crate::facts::Span {
                start: content.start(),
                end: content.end(),
            }),
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
/// buffers, and NFA construction is far too expensive for that. Multiple
/// named regex providers may coexist in one bowl.
static COMPILED_PATTERNS: Mutex<BTreeMap<String, Regex>> = Mutex::new(BTreeMap::new());

fn cached_embedding_pattern(pattern: &str) -> Option<Regex> {
    let mut cached = COMPILED_PATTERNS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(regex) = cached.get(pattern) {
        return Some(regex.clone());
    }
    let regex = compile_embedding_pattern(pattern).ok()?;
    cached.insert(pattern.to_string(), regex.clone());
    Some(regex)
}
