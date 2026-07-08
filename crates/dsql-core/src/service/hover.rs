//! Hover service: request components, request enrichment, and finalization
//! of the candidate facts the language entities contribute.
//!
//! The pipeline is ordered by phases: enrichment (Complete) resolves the
//! request's file through a bound join on `FilePath`; each entity's hover
//! systems (Complete, registered through `HoverStage`) insert
//! [`HoverCandidate`] facts for spans containing the cursor; the finalizer
//! (Cleanup) picks the highest-priority candidate and writes [`HoverInfo`]
//! onto the request. Arbitration is data, not call order.
//!
//! External callers drive it request/response:
//! `bowl.insert((HoverRequest, FilePath(...), Position { offset })).await
//!     .bind().take::<HoverInfo>()`.

use bowl::{
    Bowl, Commands, Component, Entity, Eq as BowlEq, Phase, Query, SystemExt, SystemParam, View,
    Where, With,
};

use crate::facts::Span;
use crate::source::{FilePath, SourceText};

/// Marks an entity as a hover request; pair with [`FilePath`] and
/// [`Position`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverRequest;

/// Byte offset of the cursor inside the requested file.
#[derive(Debug, Component, Hash)]
#[component(hash)]
pub struct Position {
    pub offset: usize,
}

/// The answer, written onto the request entity by the finalizer.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct HoverInfo(pub String);

/// Marker stamped on every hover request, resolvable or not. Downstream
/// systems key on enrichment outputs so phase ordering covers them all.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverEnriched;

/// The file entity a hover request resolved to, stamped by enrichment.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverFile(pub Entity);

/// One entity's answer for one hover request. The finalizer picks the
/// highest priority; see [`priority`] for the bands.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverCandidate {
    pub request: Entity,
    pub priority: u8,
    pub text: String,
}

/// Priority bands for hover candidates: the more specific the matched
/// construct, the higher it ranks.
pub mod priority {
    /// A variable occurrence under the cursor.
    pub const VARIABLE: u8 = 40;
    /// A fragment spread name under the cursor.
    pub const SPREAD: u8 = 30;
    /// A field selection name under the cursor.
    pub const FIELD: u8 = 20;
    /// A definition name under the cursor.
    pub const DEFINITION: u8 = 10;
}

/// Registers enrichment and finalization; entity candidate systems register
/// themselves through `HoverStage`.
pub(crate) async fn register_hover_pipeline(bowl: &Bowl) {
    bowl.add_system(stamp_hover_requests.run_during(Phase::Complete))
        .await;
    bowl.add_system(resolve_hover_requests.run_during(Phase::Complete))
        .await;
    bowl.add_system(finalize_hover.run_during(Phase::Cleanup))
        .await;
}

async fn stamp_hover_requests(
    query: Query<(Entity, &Position), With<HoverRequest>>,
    mut commands: Commands,
) {
    let (request, _position) = query.item();
    commands.entity(request).insert(HoverEnriched);
}

/// The request's `FilePath` binds to the file entity carrying the equal
/// path — one invocation per (request, matching file).
async fn resolve_hover_requests(
    query: Query<(Entity, &FilePath, &Position), With<HoverRequest>>,
    file: Query<(Entity, &SourceText), Where<BowlEq<FilePath>>>,
    mut commands: Commands,
) {
    let (request, _path, _position) = query.item();
    let (file_entity, _text) = file.item();

    commands.entity(request).insert(HoverFile(file_entity));
}

/// Everything the finalizer reads, bundled so the signature stays flat as
/// entities grow.
#[derive(SystemParam)]
struct HoverOutcome<'a> {
    candidates: View<'a, (Entity, &'a HoverCandidate)>,
    files: View<'a, (Entity, &'a HoverFile)>,
}

async fn finalize_hover(
    query: Query<(Entity, &HoverEnriched), With<HoverRequest>>,
    outcome: HoverOutcome<'_>,
    mut commands: Commands,
) {
    let (request, _enriched) = query.item();

    if !outcome.files.iter().any(|(entity, _)| entity == request) {
        commands
            .entity(request)
            .insert(HoverInfo("unknown file".to_string()));
        return;
    }

    let best = outcome
        .candidates
        .iter()
        .filter(|(_, candidate)| candidate.request == request)
        .max_by_key(|(_, candidate)| candidate.priority)
        .map(|(_, candidate)| candidate.text.clone());

    let message = best.unwrap_or_else(|| "no information at position".to_string());
    commands.entity(request).insert(HoverInfo(message));
}

/// Whether `span` of a fact in `file` matches a request resolved to
/// (`request_file`, `offset`). Shared by entity candidate systems.
pub fn span_matches(span: Span, file: Entity, request_file: Entity, offset: usize) -> bool {
    file == request_file && span.start <= offset && offset < span.end
}
