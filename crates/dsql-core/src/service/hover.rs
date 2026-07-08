//! Hover service: request components, request enrichment, and arbitration
//! of the candidate facts the language entities contribute.
//!
//! The pipeline needs exactly one phase barrier, and arbitration is a
//! commutative fold, not call order:
//!
//! 1. `resolve_hover_requests` (Evaluate, tracked inputs only) is an
//!    *outer* join: the request's `FilePath` pairs with the file carrying
//!    the equal path, and a request matching no file still runs once with
//!    `None` — so one system seeds the whole answer scaffold
//!    ([`RequestKey`], [`HoverRank`], the fallback [`HoverInfo`]) for
//!    matched and unmatched requests alike.
//! 2. Each entity's hover systems (Complete, registered through
//!    `HoverStage`) read the enriched request plus their own facts
//!    *ambiently* and insert [`HoverCandidate`] facts addressed by an
//!    equal [`RequestKey`]. The ambient reads of lowered facts are why
//!    they sit behind the Complete barrier.
//! 3. `arbitrate_hover` (also Complete) consumes candidates *tracked*: the
//!    [`RequestKey`] join yields one invocation per (request, candidate)
//!    pair, each monotonically upgrading the request's answer in place
//!    when its candidate outranks the current one. A max-fold commutes, so
//!    pair order is irrelevant, and tracked consumption replans pairs as
//!    candidates commit — no barrier after the candidate systems, and no
//!    settle-phase answering (settle inserts defer to the next run).
//!
//! External callers drive it request/response:
//! `bowl.insert((HoverRequest, FilePath(...), Position { offset })).await
//!     .bind().take::<HoverInfo>()`.

use bowl::{
    Bowl, Commands, Component, Entity, Eq as BowlEq, MutRef, Phase, Query, SystemExt, Where, With,
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

/// The answer, upgraded in place on the request entity by arbitration.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct HoverInfo(pub String);

/// The request's own id as a join key: candidates carry an equal key, so
/// arbitration pairs each request with exactly its own candidates.
#[derive(Component, Hash, Debug, Clone, Copy, PartialEq, Eq)]
#[component(hash)]
pub struct RequestKey(pub Entity);

/// Priority of the request's current [`HoverInfo`]. Answers only ever
/// upgrade (strictly greater), which makes arbitration order-independent.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverRank(pub u8);

/// Marker stamped on every hover request, resolvable or not. Candidate
/// systems key on enrichment outputs so the phase barrier covers them all.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverEnriched;

/// The file entity a hover request resolved to, stamped by enrichment.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverFile(pub Entity);

/// One entity's answer for one hover request, addressed by an equal
/// [`RequestKey`]; see [`priority`] for the bands.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverCandidate {
    pub priority: u8,
    pub text: String,
}

/// Priority bands for hover answers: the more specific the matched
/// construct, the higher. The zero and fallback bands are the enrichment
/// scaffold every candidate outranks.
pub mod priority {
    /// A variable occurrence under the cursor.
    pub const VARIABLE: u8 = 40;
    /// A fragment spread name under the cursor.
    pub const SPREAD: u8 = 30;
    /// A field selection name under the cursor.
    pub const FIELD: u8 = 20;
    /// A definition name under the cursor.
    pub const DEFINITION: u8 = 10;
    /// The request resolved to a file but no candidate answered.
    pub const RESOLVED: u8 = 1;
    /// Initial scaffold: the request did not even resolve to a file.
    pub const NONE: u8 = 0;
}

/// Registers enrichment and arbitration; entity candidate systems register
/// themselves through `HoverStage`.
pub(crate) async fn register_hover_pipeline(bowl: &Bowl) {
    bowl.add_system(resolve_hover_requests).await;
    bowl.add_system(arbitrate_hover.run_during(Phase::Complete))
        .await;
}

/// The file side of the enrichment outer join: matched per equal path, or
/// `None` exactly once for a request matching no file.
type FileMatch<'a> = Option<Query<(Entity, &'a SourceText), Where<BowlEq<FilePath>>>>;

/// Outer join: the request's `FilePath` pairs with the file carrying the
/// equal path, or runs once with `None` — one system seeds the scaffold
/// for matched and unmatched requests alike.
async fn resolve_hover_requests(
    query: Query<(Entity, &FilePath, &Position), With<HoverRequest>>,
    file: FileMatch<'_>,
    mut commands: Commands,
) {
    let (request, _path, _position) = query.item();
    commands.entity(request).insert(RequestKey(request));
    commands.entity(request).insert(HoverEnriched);

    let Some(file) = file else {
        commands.entity(request).insert(HoverRank(priority::NONE));
        commands
            .entity(request)
            .insert(HoverInfo("unknown file".to_string()));
        return;
    };

    let (file_entity, _text) = file.item();
    commands.entity(request).insert(HoverFile(file_entity));
    commands
        .entity(request)
        .insert(HoverRank(priority::RESOLVED));
    commands
        .entity(request)
        .insert(HoverInfo("no information at position".to_string()));
}

/// One request row keyed for arbitration, with its upgradable answer.
type ArbitrationRow<'a> = (
    Entity,
    &'a RequestKey,
    MutRef<'a, HoverRank>,
    MutRef<'a, HoverInfo>,
);

/// Arbitration: one invocation per (request, candidate) pair via the
/// [`RequestKey`] join, each monotonically upgrading the request's answer
/// when its candidate outranks the current one.
async fn arbitrate_hover(
    query: Query<ArbitrationRow<'_>, With<HoverRequest>>,
    candidate: Query<(Entity, &HoverCandidate), Where<BowlEq<RequestKey>>>,
) {
    let (_request, _key, mut rank, mut info) = query.item();
    let (_candidate_entity, candidate) = candidate.item();

    if candidate.priority > rank.0 {
        rank.0 = candidate.priority;
        info.0 = candidate.text.clone();
    }
}

/// Whether `span` of a fact in `file` matches a request resolved to
/// (`request_file`, `offset`). Shared by entity candidate systems.
pub fn span_matches(span: Span, file: Entity, request_file: Entity, offset: usize) -> bool {
    file == request_file && span.start <= offset && offset < span.end
}
