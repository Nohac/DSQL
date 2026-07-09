//! Name resolution as a tracked fixed point over the selection tree's
//! maintained relationships.
//!
//! A selection's meaning — which table its set resolves against, whether
//! its name is a column, a relation, or nothing — depends on its parent's
//! meaning and the catalog. That recursion is expressed as joins, not a
//! walk: roots pair definitions with their [`Children`], and each nested
//! step pairs a field's own resolution fact (through the maintained
//! [`FieldResolutions`] inverse) with its children. Every input is
//! tracked, so the whole stage runs at Evaluate and re-derives only the
//! chains below a change — no ambient views, no phase barrier, no
//! whole-project gather.
//!
//! Resolutions are *separate* derived entities, never components stamped
//! onto the syntax entities: stamping would bump the syntax entities'
//! revisions and retire every diagnostic anchored to them without
//! anything re-deriving those. Each fact carries [`BelongsToFile`] as its
//! join key and denormalizes the spans consumers need, plus a
//! [`ResolutionOf`] edge back to its field so the nested step can join
//! through the engine-maintained inverse.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, In, Query, Where};

use crate::catalog::{
    CatalogSnapshot, ColumnId, FieldCheckResult, FieldRef, ForeignKeyId, TableId, TableRef,
    TableResolution,
};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::field_selection::FieldSel;
use crate::facts::{BelongsToFile, Children, Span};

/// What one selection's name means in its context: a derived fact entity
/// carrying everything span-addressed consumers need.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedSelection {
    /// The field-selection entity this resolution is about.
    pub field: Entity,
    /// The selected name (full qualified-name text).
    pub name: String,
    /// The reference as written, including an explicit `->selector`.
    pub written: String,
    /// Span of the selected name in its document.
    pub name_span: Span,
    /// Span of the output alias, when written as `alias: field`.
    pub alias_span: Option<Span>,
    /// The query root's table, threaded down for `~` path anchors.
    pub root: Option<TableId>,
    /// The table the name resolved against — `None` for query roots
    /// (which name tables directly) and for selections whose context
    /// never resolved.
    pub context: Option<TableId>,
    pub target: SelectionTarget,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SelectionTarget {
    /// A query root naming a table.
    Table(TableId),
    /// A scalar column of the context table.
    Column(ColumnId),
    /// A relation stepping to another table.
    Relation {
        table: TableId,
        foreign_key: ForeignKeyId,
        /// The foreign-key selector, for display.
        selector: String,
    },
    /// The name (or its context) resolved to nothing; the checks report
    /// why.
    Unresolved,
}

impl SelectionTarget {
    /// The table this selection's own children and clauses resolve
    /// against, if any.
    fn child_context(&self) -> Option<TableId> {
        match self {
            SelectionTarget::Table(table) => Some(*table),
            SelectionTarget::Relation { table, .. } => Some(*table),
            SelectionTarget::Column(_) | SelectionTarget::Unresolved => None,
        }
    }
}

/// Relationship edge from a resolution fact to the field it resolves; the
/// engine maintains [`FieldResolutions`] on the field, which is what lets
/// the nested step join a field row with its own resolution.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
#[relationship(target = FieldResolutions)]
pub struct ResolutionOf(pub Entity);

/// Engine-maintained inverse of [`ResolutionOf`] on each resolved field.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ResolutionOf)]
pub struct FieldResolutions(pub Vec<Entity>);

/// The tables one clause's expressions resolve against: `None` when the
/// owning selection never resolved.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedClause {
    /// The clause entity this resolution is about.
    pub clause: Entity,
    pub context: Option<ClauseContext>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ClauseContext {
    /// The query root's table (`~` paths anchor here).
    pub root: TableId,
    /// The clause's own table (`.` paths anchor here).
    pub table: TableId,
}

pub async fn register_resolution(bowl: &Bowl) {
    bowl.add_system(resolve_roots).await;
    bowl.add_system(resolve_nested).await;
    bowl.add_system(resolve_clauses).await;
}

/// Emits one resolution fact.
#[expect(clippy::too_many_arguments, reason = "one emission site, all context")]
fn emit(
    commands: &mut Commands,
    catalog_entity: Entity,
    file: Entity,
    field: Entity,
    selection: &FieldSel,
    root: Option<TableId>,
    context: Option<TableId>,
    target: SelectionTarget,
) {
    let written = match &selection.relation_path {
        Some(path) => format!("{}->{path}", selection.name),
        None => selection.name.clone(),
    };
    commands.insert((
        DerivedFrom::many([field, catalog_entity]),
        BelongsToFile(file),
        ResolutionOf(field),
        ResolvedSelection {
            field,
            name: selection.name.clone(),
            written,
            name_span: selection.name_span,
            alias_span: selection.alias_span,
            root,
            context,
            target,
        },
    ));
}

/// Resolves the direct children of each definition: query roots name
/// tables; fragment-body roots resolve against the declared target. One
/// tracked invocation per (definition, child-field) pair.
async fn resolve_roots(
    defs: Query<(
        Entity,
        &DefDecl,
        Option<&FragmentTarget>,
        &BelongsToFile,
        &Children,
    )>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    roots: Query<(Entity, &FieldSel), Where<In<Children>>>,
    mut commands: Commands,
) {
    let (_, decl, target, file, _children) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();
    let (field_entity, field) = roots.item();

    match decl.kind {
        DefKind::Query => {
            let (root, resolved) = match catalog.resolve_table_ref_for(TableRef::parse(&field.name))
            {
                TableResolution::Found(table) => (Some(table.id), SelectionTarget::Table(table.id)),
                TableResolution::NotFound { .. } | TableResolution::Ambiguous { .. } => {
                    (None, SelectionTarget::Unresolved)
                }
            };
            emit(
                &mut commands,
                catalog_entity,
                file.0,
                field_entity,
                field,
                root,
                None,
                resolved,
            );
        }
        DefKind::Fragment => {
            let table = target
                .and_then(|target| catalog.table_ref_for(TableRef::parse(&target.name)))
                .map(|table| table.id);
            let (root, context, resolved) = match table {
                Some(table) => (
                    Some(table),
                    Some(table),
                    resolve_reference(catalog, table, field),
                ),
                // Unresolvable target: the whole body is unresolved.
                None => (None, None, SelectionTarget::Unresolved),
            };
            emit(
                &mut commands,
                catalog_entity,
                file.0,
                field_entity,
                field,
                root,
                context,
                resolved,
            );
        }
    }
}

/// Resolves each field against its parent's resolution: one tracked
/// invocation per (parent, parent-resolution, child) triple, joined
/// through the maintained inverses — the recursive walk as a fixed point.
async fn resolve_nested(
    parents: Query<(
        Entity,
        &FieldSel,
        &Children,
        &FieldResolutions,
        &BelongsToFile,
    )>,
    resolution: Query<(Entity, &ResolvedSelection), Where<In<FieldResolutions>>>,
    children: Query<(Entity, &FieldSel), Where<In<Children>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let (_, _, _, _, file) = parents.item();
    let (_, parent_resolved) = resolution.item();
    let (field_entity, field) = children.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let (context, resolved) = match parent_resolved.target.child_context() {
        Some(table) => (Some(table), resolve_reference(catalog, table, field)),
        // Under a scalar or unresolved parent nothing resolves.
        None => (None, SelectionTarget::Unresolved),
    };
    let root = parent_resolved.root.filter(|_| context.is_some());
    emit(
        &mut commands,
        catalog_entity,
        file.0,
        field_entity,
        field,
        root,
        context,
        resolved,
    );
}

/// Resolves each clause against its owning selection's target table: one
/// tracked invocation per (field, field-resolution, clause) triple.
async fn resolve_clauses(
    parents: Query<(
        Entity,
        &FieldSel,
        &Children,
        &FieldResolutions,
        &BelongsToFile,
    )>,
    resolution: Query<(Entity, &ResolvedSelection), Where<In<FieldResolutions>>>,
    clauses: Query<(Entity, &ClauseFact), Where<In<Children>>>,
    mut commands: Commands,
) {
    let (field_entity, _, _, _, file) = parents.item();
    let (_, parent_resolved) = resolution.item();
    let (clause_entity, _clause) = clauses.item();

    let context = match (parent_resolved.root, parent_resolved.target.child_context()) {
        (Some(root), Some(table)) => Some(ClauseContext { root, table }),
        _ => None,
    };
    commands.insert((
        DerivedFrom::many([field_entity, clause_entity]),
        BelongsToFile(file.0),
        ResolvedClause {
            clause: clause_entity,
            context,
        },
    ));
}

/// One name against one context table.
fn resolve_reference(
    catalog: &crate::catalog::Catalog,
    table: TableId,
    field: &FieldSel,
) -> SelectionTarget {
    let reference = FieldRef {
        target: TableRef::parse(&field.name),
        selector: field.relation_path.as_deref(),
    };
    match catalog.check_field_ref(table, reference) {
        FieldCheckResult::Column(column) => SelectionTarget::Column(column.id),
        FieldCheckResult::Relation(relation) => SelectionTarget::Relation {
            table: relation.table.id,
            foreign_key: relation.foreign_key.id,
            selector: relation.selector.clone(),
        },
        FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {
            SelectionTarget::Unresolved
        }
    }
}
