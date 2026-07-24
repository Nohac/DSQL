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
    Commands, Component, DerivedFrom, Entity, MutRef, Phase, Query, Registrar, SystemExt, View,
    With,
};
use regex::Regex;

use crate::entities::document::ParsedFile;
use crate::schema::dsql_schema;
use crate::source::{
    AnalysisResidency, BelongsToHost, CallsiteSpan, ContentSpan, DsqlDocument, EmbeddingHost,
    ExtractionResolver, OpenBuffer, ResolutionScope, ScopeImports, SourceOffset, SourceText,
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

/// The semantic value of one embedded host expression.
///
/// This fact is the single owner of the exactly-one-definition rule consumed
/// by diagnostics and build-daemon callsite publication.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct ResolvedEmbeddedExpression(pub EmbeddedExpressionResolution);

/// Resolution outcome for one [`ResolvedEmbeddedExpression`].
#[derive(Debug, Hash)]
pub enum EmbeddedExpressionResolution {
    /// The sole top-level definition, which may be a query or fragment.
    Target(Entity),
    /// The embedded document contains no top-level definition.
    Empty,
    /// The embedded document contains more than one top-level definition.
    MultipleDefinitions(usize),
}

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
    reg.system(resolve_embedded_expressions.run_during(Phase::Complete));
    reg.system(check_embedded_expressions.run_during(bowl::Phase::Complete));
}

/// Resolves an embedded expression to its sole top-level definition.
async fn resolve_embedded_expressions(
    regions: Query<(Entity, &CallsiteSpan, &SourceText), With<DsqlDocument>>,
    // The index moves when definitions appear, change kind/name, or vanish.
    _index: Query<(Entity, &crate::entities::definition::DefIndex)>,
    defs: View<
        '_,
        (
            Entity,
            &crate::entities::definition::DefDecl,
            &crate::facts::BelongsToFile,
        ),
    >,
    mut commands: Commands<(crate::schema::dsql_schema::ResolvedEmbeddedExpression,)>,
) {
    let (region, _callsite, _source) = regions.item();
    let definitions: Vec<Entity> = defs
        .iter()
        .filter(|(_, _, file)| file.0 == region)
        .map(|(definition, _, _)| definition)
        .collect();
    let resolution = match definitions.as_slice() {
        [] => EmbeddedExpressionResolution::Empty,
        [definition] => EmbeddedExpressionResolution::Target(*definition),
        definitions => EmbeddedExpressionResolution::MultipleDefinitions(definitions.len()),
    };
    let derived_from = match resolution {
        EmbeddedExpressionResolution::Target(definition) => DerivedFrom::many([region, definition]),
        EmbeddedExpressionResolution::Empty
        | EmbeddedExpressionResolution::MultipleDefinitions(_) => DerivedFrom::new(region),
    };
    commands.insert((
        ResolvedEmbeddedExpression(resolution),
        crate::facts::BelongsToFile(region),
        derived_from,
    ));
}

/// Emits diagnostics for embedded expressions without exactly one definition.
async fn check_embedded_expressions(
    _: Query<Entity, With<crate::facts::DiagnosticsDemand>>,
    imports: Query<(Entity, &ScopeImports)>,
    resolved: Query<(
        Entity,
        &ResolvedEmbeddedExpression,
        &crate::facts::BelongsToFile,
    )>,
    regions: View<'_, (Entity, &ResolutionScope, &ParsedFile)>,
    mut commands: Commands<(crate::schema::dsql_schema::Diagnostic,)>,
) {
    use crate::facts::{
        DiagnosticCode, DiagnosticFacts, DiagnosticSource, Severity, emit_diagnostic,
    };

    let (imports_entity, imports) = imports.item();
    let (resolution, resolved, file) = resolved.item();
    let region = regions
        .iter()
        .find(|(region, _, _)| *region == file.0)
        .map(|(_, scope, parsed)| (scope, parsed));
    let span = region
        .map(|(_, parsed)| crate::facts::Span {
            start: 0,
            end: parsed.source.len(),
        })
        .unwrap_or(crate::facts::Span { start: 0, end: 0 });
    let shape_message = match resolved.0 {
        EmbeddedExpressionResolution::Target(_) => None,
        EmbeddedExpressionResolution::Empty => Some(
            "embedded dsql expression defines no top-level definition; exactly one is required"
                .to_string(),
        ),
        EmbeddedExpressionResolution::MultipleDefinitions(count) => Some(format!(
            "embedded dsql expression defines {count} top-level definitions; exactly one is required"
        )),
    };
    if let Some(message) = shape_message {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(resolution),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::EmbeddedExpressionShape,
                message,
            },
        );
    }
    let scope = region.map(|(scope, _)| scope);
    if let Some(scope) = scope
        && imports.0.contains_key(&scope.0)
        && !imports.is_generation_target(&scope.0)
    {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([resolution, imports_entity]),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::EmbeddedNonTerminalScope,
                message: format!(
                    "embedded dsql expression belongs to non-terminal resolution scope `{}`; \
                     embedded regions require one terminal generation target",
                    scope.0
                ),
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
