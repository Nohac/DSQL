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
//!    ([`RequestKey`] and the fallback [`HoverInfo`]) for matched and
//!    unmatched requests alike.
//! 2. Each entity's hover systems (registered in their entity's
//!    `register`) join the enriched request with their own facts and
//!    insert [`HoverCandidate`] facts addressed by an equal
//!    [`RequestKey`]. Fully tracked candidates run phase-free; only the
//!    ones still reading lowered facts ambiently sit behind Complete.
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
    Commands, Component, Entity, Eq as BowlEq, MutRef, Phase, Query, Registrar, SystemExt, View,
    Where, With,
};

use crate::schema::dsql_schema;
use crate::source::{BelongsToHost, FilePath, SourceOffset, SourceText};

/// Marks an entity as a hover request; pair with [`FilePath`] and
/// [`Position`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverRequest;

/// Byte offset of the cursor inside the requested file, as the caller
/// sent it — host coordinates when the file is an embedding host.
#[derive(Debug, Component, Hash)]
#[component(hash)]
pub struct Position {
    pub offset: usize,
}

/// The cursor rebased into the document the request actually targets:
/// equal to [`Position`] for plain files, region-relative for a cursor
/// inside an extracted region. Derived by enrichment; candidate systems
/// read this, never [`Position`].
#[derive(Debug, Component, Hash, Clone, Copy)]
#[component(hash)]
pub struct Cursor(pub usize);

/// The answer, upgraded in place on the request entity by arbitration.
/// `priority` tells callers whether anything actually answered: at or
/// below [`priority::RESOLVED`] the text is scaffold fallback, and editor
/// integrations should show nothing.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct HoverInfo {
    pub text: String,
    pub priority: u8,
}

/// The request's own id as a join key: candidates carry an equal key, so
/// arbitration pairs each request with exactly its own candidates.
#[derive(Component, Hash, Debug, Clone, Copy, PartialEq, Eq)]
#[component(hash)]
pub struct RequestKey(pub Entity);

/// Marker stamped on every hover request, resolvable or not. Candidate
/// systems key on enrichment outputs so the phase barrier covers them all.
#[derive(Component, Hash)]
#[component(hash)]
pub struct HoverEnriched;

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
/// themselves in their entity's `LanguageEntity::register`.
pub(crate) fn register_hover_pipeline(reg: &mut Registrar<'_>) {
    // Enrichment reads the region view ambiently (Evaluate-derived):
    // behind the Complete barrier.
    reg.system(resolve_hover_requests.run_during(Phase::Complete));
    reg.system(arbitrate_hover.run_during(Phase::Complete));
}

/// The file side of the enrichment outer join: matched per equal path, or
/// `None` exactly once for a request matching no file.
pub(crate) type FileMatch<'a> = Option<Query<(Entity, &'a SourceText), Where<BowlEq<FilePath>>>>;

/// The document a host-coordinate cursor lands in: the containing region
/// (with the cursor rebased) when `file` is an embedding host, otherwise
/// the file itself.
pub(crate) fn map_cursor(
    regions: &View<'_, (Entity, &BelongsToHost, &SourceOffset, &SourceText)>,
    file: Entity,
    offset: usize,
) -> (Entity, usize) {
    regions
        .iter()
        .find(|(_, host, start, text)| {
            host.0 == file && offset >= start.0 && offset < start.0 + text.rope().len()
        })
        .map(|(region, _, start, _)| (region, offset - start.0))
        .unwrap_or((file, offset))
}

/// Outer join: the request's `FilePath` pairs with the file carrying the
/// equal path, or runs once with `None` — one system seeds the scaffold
/// for matched and unmatched requests alike. Cursors into embedding hosts
/// rebase onto the containing region, which is why this reads the region
/// view ambiently and sits at Complete.
async fn resolve_hover_requests(
    query: Query<(Entity, &FilePath, &Position), With<HoverRequest>>,
    file: FileMatch<'_>,
    regions: View<'_, (Entity, &BelongsToHost, &SourceOffset, &SourceText)>,
    mut commands: Commands<(dsql_schema::HoverAnswer,)>,
) {
    let (request, _path, position) = query.item();
    commands.entity(request).insert(RequestKey(request));
    commands.entity(request).insert(HoverEnriched);

    let Some(file) = file else {
        commands.entity(request).insert(HoverInfo {
            text: "unknown file".to_string(),
            priority: priority::NONE,
        });
        return;
    };

    // The resolved document lands as `BelongsToFile` with the rebased
    // cursor: candidate systems join their per-file facts against it
    // instead of scanning views.
    let (file_entity, _text) = file.item();
    let (target, cursor) = map_cursor(&regions, file_entity, position.offset);
    commands
        .entity(request)
        .insert(crate::facts::BelongsToFile(target));
    commands.entity(request).insert(Cursor(cursor));
    commands.entity(request).insert(HoverInfo {
        text: "no information at position".to_string(),
        priority: priority::RESOLVED,
    });
}

/// One request row keyed for arbitration, with its upgradable answer.
type ArbitrationRow<'a> = (Entity, &'a RequestKey, MutRef<'a, HoverInfo>);

/// Arbitration: one invocation per (request, candidate) pair via the
/// [`RequestKey`] join, each monotonically upgrading the request's answer
/// when its candidate outranks the current one.
async fn arbitrate_hover(
    query: Query<ArbitrationRow<'_>, With<HoverRequest>>,
    candidate: Query<(Entity, &HoverCandidate), Where<BowlEq<RequestKey>>>,
) {
    let (_request, _key, mut info) = query.item();
    let (_candidate_entity, candidate) = candidate.item();

    if candidate.priority > info.priority {
        info.priority = candidate.priority;
        info.text = candidate.text.clone();
    }
}
