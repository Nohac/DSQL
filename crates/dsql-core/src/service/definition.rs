//! Go-to-definition service: a request at a fragment-spread name answers
//! with the fragment definition's name span.
//!
//! Same shape as hover — request, enrichment (shared: this pipeline reads
//! the hover enrichment components when both mark the entity), candidates,
//! finalizer — but the only contributor today is the spread → definition
//! link, which follows the `SpreadResolution` facts the fragment-spread
//! entity derives.

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
    bowl.add_system(resolve_definition_requests.run_during(Phase::Complete))
        .await;
    bowl.add_system(answer_spread_definitions.run_during(Phase::Cleanup))
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

/// Follows the spread under the cursor to its fragment definition. Runs at
/// Cleanup so the resolution facts of this generation are all present.
async fn answer_spread_definitions(
    query: Query<(Entity, &DefinitionFile, &Position), With<DefinitionRequest>>,
    spreads: View<'_, (Entity, &SpreadDecl, &BelongsToFile)>,
    resolutions: View<'_, (Entity, &SpreadResolution)>,
    defs: View<'_, (Entity, &DefDecl, &BelongsToFile)>,
    mut commands: Commands,
) {
    let (request, file, position) = query.item();

    let Some((spread_entity, _, _)) = spreads.iter().find(|(_, spread, spread_file)| {
        spread_file.0 == file.0
            && spread.name_span.start <= position.offset
            && position.offset < spread.name_span.end
    }) else {
        return;
    };

    let Some((_, resolution)) = resolutions
        .iter()
        .find(|(_, resolution)| resolution.spread == spread_entity)
    else {
        return;
    };

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
