//! Go-to-definition service: a request at a fragment-spread name answers
//! with the fragment definition's name span.
//!
//! The answer follows the `SpreadResolution` facts the fragment-spread
//! entity derives, consumed *tracked* so pairs replan as resolutions
//! commit — resolutions derive at Complete, and a same-phase ambient view
//! of them would race. The (request × resolution) product is unkeyed
//! (nothing equal joins a cursor offset to a resolution), so each pair
//! filters in its body; requests are transient and few, so the product
//! stays small.

use bowl::{
    Bowl, Commands, Component, Entity, Eq as BowlEq, Phase, Query, SystemExt, View, Where, With,
};

use crate::entities::definition::DefDecl;
use crate::entities::fragment_spread::{SpreadDecl, SpreadResolution};
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

/// The request's resolved file, stamped by enrichment.
#[derive(Component, Hash)]
#[component(hash)]
struct DefinitionFile(Entity);

pub(crate) async fn register_definition_pipeline(bowl: &Bowl) {
    bowl.add_system(resolve_definition_requests).await;
    bowl.add_system(answer_spread_definitions.run_during(Phase::Complete))
        .await;
}

async fn resolve_definition_requests(
    query: Query<(Entity, &FilePath, &Position), With<DefinitionRequest>>,
    file: Query<(Entity, &SourceText), Where<BowlEq<FilePath>>>,
    mut commands: Commands,
) {
    let (request, _path, _position) = query.item();
    let (file_entity, _text) = file.item();
    commands.entity(request).insert(DefinitionFile(file_entity));
}

/// Follows the spread under the cursor to its fragment definition: one
/// tracked invocation per (request, resolution) pair, answering when the
/// resolution's spread is the one under the cursor. Lowered spread and
/// definition facts are viewed ambiently (Evaluate output, safe behind
/// the Complete barrier); the Complete-derived resolutions are the tracked
/// input.
async fn answer_spread_definitions(
    query: Query<(Entity, &DefinitionFile, &Position), With<DefinitionRequest>>,
    resolutions: Query<(Entity, &SpreadResolution)>,
    spreads: View<'_, (Entity, &SpreadDecl, &BelongsToFile)>,
    defs: View<'_, (Entity, &DefDecl, &BelongsToFile)>,
    mut commands: Commands,
) {
    let (request, file, position) = query.item();
    let (_, resolution) = resolutions.item();

    let under_cursor = spreads.iter().any(|(spread_entity, spread, spread_file)| {
        spread_entity == resolution.spread
            && spread_file.0 == file.0
            && spread.name_span.start <= position.offset
            && position.offset < spread.name_span.end
    });
    if !under_cursor {
        return;
    }

    let Some((_, decl, def_file)) = defs
        .iter()
        .find(|(entity, _, _)| *entity == resolution.fragment)
    else {
        return;
    };

    commands.entity(request).insert(DefinitionTarget {
        file: def_file.0,
        span: decl.name_span,
    });
}
