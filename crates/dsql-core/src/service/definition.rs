//! Go-to-definition service: fragment spreads answer source locations;
//! resolved schema names answer stable catalog identities.
//!
//! Enrichment stamps the resolved file as `BelongsToFile`, so the spread
//! lookup is a per-file bound join, and the [`ResolvedSpread`] stamp on
//! the spread row carries the target — a fully tracked pipeline, no
//! phase barrier.

use crate::schema::dsql_schema;
use bowl::{
    Commands, Component, Entity, Eq as BowlEq, Phase, Query, Registrar, SystemExt, Where, With,
};

use crate::catalog::{Catalog, CatalogSnapshot, CatalogSupport, ColumnId, RelationId, TableId};
use crate::entities::fragment_spread::{ResolvedSpread, ResolvedSpreadNavigation};
use crate::facts::{BelongsToFile, Span};
use crate::resolution::{
    PathTerminal, ResolvedClause, ResolvedFragmentTarget, ResolvedSelection, ResolvedTableTarget,
    SelectionTarget,
};
use crate::service::hover::{Cursor, FileMatch, Position, map_cursor};
use crate::source::FilePath;

/// Marks an entity as a go-to-definition request; pair with [`FilePath`]
/// and [`Position`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct DefinitionRequest;

/// A schema definition identified independently of its YAML presentation.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum CatalogDefinition {
    /// A table definition in one schema.
    Table {
        schema: String,
        table: String,
        support: Option<CatalogSupport>,
    },
    /// A column definition in one table.
    Column {
        schema: String,
        table: String,
        column: String,
        support: Option<CatalogSupport>,
    },
    /// A directional effective relationship.
    Relation {
        schema: String,
        table: String,
        name: String,
        support: Option<CatalogSupport>,
    },
}

/// The answer written onto a definition request.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub enum DefinitionTarget {
    /// A definition inside a dsql source entity.
    Source {
        /// The file entity holding the definition.
        file: Entity,
        /// The definition's name span within that file.
        span: Span,
    },
    /// A table or column in the introspected catalog.
    Catalog(CatalogDefinition),
}

pub(crate) fn register_definition_pipeline(reg: &mut Registrar<'_>) {
    // Enrichment reads the region view ambiently: behind Complete.
    reg.system(resolve_definition_requests.run_during(Phase::Complete));
    reg.system(answer_spread_definitions);
    reg.system(answer_selection_definitions);
    reg.system(answer_clause_definitions);
    reg.system(answer_fragment_target_definitions);
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
/// tracked invocation per (request, spread-in-file) pair. Navigation is a
/// separate component so definition-span edits do not invalidate semantic
/// spread consumers.
async fn answer_spread_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    spreads: Query<
        (Entity, &ResolvedSpread, &ResolvedSpreadNavigation),
        Where<BowlEq<BelongsToFile>>,
    >,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved, navigation) = spreads.item();

    if !resolved.name_span.contains(cursor.0) {
        return;
    }
    commands.entity(request).insert(DefinitionTarget::Source {
        file: navigation.file,
        span: navigation.name_span,
    });
}

/// Answers a resolved field name: query roots and relations target tables;
/// scalar selections target columns.
async fn answer_selection_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    selections: Query<(Entity, &ResolvedSelection), Where<BowlEq<BelongsToFile>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved) = selections.item();
    let (_, snapshot) = catalog.item();
    if !resolved.name_span.contains(cursor.0) {
        return;
    }
    let target = match &resolved.target {
        SelectionTarget::Table(table) => table_definition(snapshot.catalog(), *table),
        SelectionTarget::Relation {
            table, relation, ..
        } => relation_definition(snapshot.catalog(), *table, *relation),
        SelectionTarget::Column(column) => column_definition(snapshot.catalog(), *column),
        SelectionTarget::Unresolved => None,
    };
    if let Some(target) = target {
        commands
            .entity(request)
            .insert(DefinitionTarget::Catalog(target));
    }
}

/// Answers relation steps, terminal columns, and order-by columns from one
/// resolved clause without re-resolving expression text.
async fn answer_clause_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    clauses: Query<(Entity, &ResolvedClause), Where<BowlEq<BelongsToFile>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved) = clauses.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let path_target = resolved.paths.iter().find_map(|path| {
        path.relations
            .iter()
            .find(|relation| relation.span.contains(cursor.0))
            .and_then(|relation| relation_definition(catalog, relation.table, relation.relation))
            .or_else(|| match &path.terminal {
                PathTerminal::Column { span, column, .. } if span.contains(cursor.0) => {
                    column_definition(catalog, *column)
                }
                PathTerminal::Column { .. } | PathTerminal::Failed | PathTerminal::OutOfScope => {
                    None
                }
            })
    });
    let target = path_target.or_else(|| {
        resolved
            .order_items
            .iter()
            .find(|item| item.span.contains(cursor.0))
            .and_then(|item| item.column)
            .and_then(|column| column_definition(catalog, column))
    });
    if let Some(target) = target {
        commands
            .entity(request)
            .insert(DefinitionTarget::Catalog(target));
    }
}

/// Answers a fragment declaration's `on` target from the shared semantic
/// resolution fact used by its diagnostics.
async fn answer_fragment_target_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    targets: Query<(Entity, &ResolvedFragmentTarget), Where<BowlEq<BelongsToFile>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved) = targets.item();
    let (_, snapshot) = catalog.item();
    if !resolved.span.contains(cursor.0) {
        return;
    }
    let ResolvedTableTarget::Table(table) = &resolved.target else {
        return;
    };
    if let Some(target) = table_definition(snapshot.catalog(), *table) {
        commands
            .entity(request)
            .insert(DefinitionTarget::Catalog(target));
    }
}

fn table_definition(catalog: &Catalog, table: TableId) -> Option<CatalogDefinition> {
    let table = catalog.table_by_id(table)?;
    Some(CatalogDefinition::Table {
        schema: table.schema.clone(),
        table: table.name.clone(),
        support: table
            .description_support
            .clone()
            .or_else(|| table.declaration.clone()),
    })
}

fn column_definition(catalog: &Catalog, column: ColumnId) -> Option<CatalogDefinition> {
    let column = catalog.column_by_id(column)?;
    Some(CatalogDefinition::Column {
        schema: column.key.schema.clone(),
        table: column.key.table.clone(),
        column: column.name.clone(),
        support: column
            .description_support
            .clone()
            .or_else(|| column.declaration.clone()),
    })
}

fn relation_definition(
    catalog: &Catalog,
    table: TableId,
    relation: RelationId,
) -> Option<CatalogDefinition> {
    let table = catalog.table_by_id(table)?;
    let relation = catalog.relation_by_id(relation)?;
    Some(CatalogDefinition::Relation {
        schema: table.schema.clone(),
        table: table.name.clone(),
        name: relation.name.clone(),
        support: relation.supports.declaration.clone(),
    })
}
