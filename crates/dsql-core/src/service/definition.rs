//! Go-to-definition service: a request at a fragment-spread name answers
//! with the fragment definition's name span.
//!
//! Enrichment stamps the resolved file as `BelongsToFile`, so the spread
//! lookup is a per-file bound join, and the [`ResolvedSpread`] stamp on
//! the spread row carries the target — a fully tracked pipeline, no
//! phase barrier.

use bowl::{Bowl, Commands, Component, Entity, Eq as BowlEq, Query, Where, With};

use crate::entities::fragment_spread::ResolvedSpread;
use crate::facts::{BelongsToFile, Span};
use crate::service::hover::Position;
use crate::source::{FilePath, SourceText};

/// Marks an entity as a go-to-definition request; pair with [`FilePath`]
/// and [`Position`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct DefinitionRequest;

/// The answer: where the definition lives, written onto the request.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct DefinitionTarget {
    /// The file entity holding the definition.
    pub file: Entity,
    /// The definition's name span within that file.
    pub span: Span,
}

pub(crate) async fn register_definition_pipeline(bowl: &Bowl) {
    bowl.add_system(resolve_definition_requests).await;
    bowl.add_system(answer_spread_definitions).await;
}

async fn resolve_definition_requests(
    query: Query<(Entity, &FilePath, &Position), With<DefinitionRequest>>,
    file: Query<(Entity, &SourceText), Where<BowlEq<FilePath>>>,
    mut commands: Commands,
) {
    let (request, _path, _position) = query.item();
    let (file_entity, _text) = file.item();
    commands.entity(request).insert(BelongsToFile(file_entity));
}

/// Follows the spread under the cursor to its fragment definition: one
/// tracked invocation per (request, resolution) pair, answering when the
/// resolution's spread is the one under the cursor. Lowered spread and
/// definition facts are viewed ambiently (Evaluate output, safe behind
/// the Complete barrier); the Complete-derived resolutions are the tracked
/// input.
/// Follows the spread under the cursor to its fragment definition: one
/// tracked invocation per (request, spread-in-file) pair, the target read
/// off the spread's [`ResolvedSpread`] stamp.
async fn answer_spread_definitions(
    query: Query<(Entity, &BelongsToFile, &Position), With<DefinitionRequest>>,
    spreads: Query<(Entity, &ResolvedSpread), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands,
) {
    let (request, _file, position) = query.item();
    let (_, resolved) = spreads.item();

    if !(resolved.name_span.start <= position.offset && position.offset < resolved.name_span.end) {
        return;
    }
    let Some(target) = &resolved.target else {
        return;
    };

    commands.entity(request).insert(DefinitionTarget {
        file: target.file,
        span: target.name_span,
    });
}
