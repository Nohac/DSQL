//! Go-to-definition service: a request at a fragment-spread name answers
//! with the fragment definition's name span.
//!
//! Enrichment stamps the resolved file as `BelongsToFile`, so the spread
//! lookup is a per-file bound join, and the [`ResolvedSpread`] stamp on
//! the spread row carries the target — a fully tracked pipeline, no
//! phase barrier.

use crate::schema::dsql_schema;
use bowl::{
    Commands, Component, Entity, Eq as BowlEq, Phase, Query, Registrar, SystemExt, Where, With,
};

use crate::entities::fragment_spread::ResolvedSpread;
use crate::facts::{BelongsToFile, Span};
use crate::service::hover::{Cursor, FileMatch, Position, map_cursor};
use crate::source::FilePath;

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

pub(crate) fn register_definition_pipeline(reg: &mut Registrar<'_>) {
    // Enrichment reads the region view ambiently: behind Complete.
    reg.system(resolve_definition_requests.run_during(Phase::Complete));
    reg.system(answer_spread_definitions);
}

async fn resolve_definition_requests(
    query: Query<(Entity, &FilePath, &Position), With<DefinitionRequest>>,
    file: FileMatch<'_>,
    regions: bowl::View<
        '_,
        (
            Entity,
            &crate::source::BelongsToHost,
            &crate::source::SourceOffset,
            &crate::entities::document::ParsedFile,
        ),
    >,
    mut commands: Commands<(dsql_schema::DefinitionEnriched,)>,
) {
    let (request, _path, position) = query.item();
    let Some(file) = file else {
        return;
    };
    let (file_entity, _text) = file.item();
    let (target, cursor) = map_cursor(&regions, file_entity, position.offset);
    commands.entity(request).insert(BelongsToFile(target));
    commands.entity(request).insert(Cursor(cursor));
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
    query: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    spreads: Query<(Entity, &ResolvedSpread), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved) = spreads.item();

    if !(resolved.name_span.start <= cursor.0 && cursor.0 < resolved.name_span.end) {
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
