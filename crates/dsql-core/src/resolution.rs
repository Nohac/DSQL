//! Name resolution as a derivation: the one cross-cutting walk.
//!
//! A selection's meaning — which table its set resolves against, whether
//! its name is a column, a relation, or nothing — depends on its ancestor
//! chain and the catalog. Before this stage existed, every consumer
//! (hover, tokens, lint, checks, planning) re-derived that meaning with
//! its own tree walk, which made all of them cross-cutting and pinned
//! them behind the Complete barrier. The resolver walks once and derives
//! semantic facts; consumers become tracked joins over those facts,
//! narrow and phase-free.
//!
//! Resolutions are *separate* derived entities, never components stamped
//! onto the syntax entities: stamping would bump the syntax entities'
//! revisions and retire every diagnostic anchored to them without
//! anything re-deriving those. Each fact carries [`BelongsToFile`] as its
//! join key and denormalizes the spans consumers need, so most never
//! touch the syntax facts at all.
//!
//! The resolver itself reads the lowered tree ambiently, so it lives at
//! Complete — the last system that must. When porridge grows relation
//! joins, the walk becomes a tracked fixed point over parent links and
//! moves to Evaluate.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Phase, Query, SystemExt};

use crate::catalog::{
    Catalog, CatalogSnapshot, ColumnId, FieldCheckResult, FieldRef, ForeignKeyId, TableId,
    TableRef, TableResolution,
};
use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::field_selection::{SelectionTree, TreeViews};
use crate::facts::{BelongsToFile, NodeKey, Span};

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
    // The walk reads lowered facts ambiently: behind the Complete barrier.
    bowl.add_system(resolve_selections.run_during(Phase::Complete))
        .await;
}

/// Walks one definition's selection tree, deriving a resolution fact for
/// every field and clause. Tracked per (definition, catalog): a schema
/// change re-resolves every definition, a text change only its file's.
async fn resolve_selections(
    defs: Query<(Entity, &DefDecl, &NodeKey, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (def_entity, decl, def_key, file) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let tree = SelectionTree::collect(&views);
    let mut walk = ResolveWalk {
        tree: &tree,
        catalog,
        catalog_entity,
        file: file.0,
        commands: &mut commands,
    };

    match decl.kind {
        DefKind::Query => walk.resolve_roots(*def_key),
        DefKind::Fragment => {
            let target = tree
                .fragments
                .iter()
                .find(|(entity, _, _, _, _)| *entity == def_entity)
                .and_then(|(_, _, target, _, _)| {
                    catalog.table_ref_for(TableRef::parse(&target.name))
                });
            match target {
                Some(table) => walk.resolve_set(table.id, table.id, *def_key),
                // Unresolvable target: the whole body is unresolved.
                None => walk.emit_unresolved(*def_key),
            }
        }
    }
}

struct ResolveWalk<'a, 'view> {
    tree: &'a SelectionTree<'view>,
    catalog: &'a Catalog,
    catalog_entity: Entity,
    file: Entity,
    commands: &'a mut Commands,
}

impl ResolveWalk<'_, '_> {
    fn emit(
        &mut self,
        field: Entity,
        selection: &crate::entities::field_selection::FieldSel,
        context: Option<TableId>,
        target: SelectionTarget,
    ) {
        let written = match &selection.relation_path {
            Some(path) => format!("{}->{path}", selection.name),
            None => selection.name.clone(),
        };
        self.commands.insert((
            DerivedFrom::many([field, self.catalog_entity]),
            BelongsToFile(self.file),
            ResolvedSelection {
                field,
                name: selection.name.clone(),
                written,
                name_span: selection.name_span,
                alias_span: selection.alias_span,
                context,
                target,
            },
        ));
    }

    fn resolve_roots(&mut self, def_key: NodeKey) {
        let roots: Vec<_> = self
            .tree
            .fields_under(def_key)
            .map(|(entity, field, key, _)| (*entity, *field, *key))
            .collect();
        for (entity, field, key) in roots {
            match self
                .catalog
                .resolve_table_ref_for(TableRef::parse(&field.name))
            {
                TableResolution::Found(table) => {
                    let table_id = table.id;
                    self.emit(entity, field, None, SelectionTarget::Table(table_id));
                    self.resolve_clauses(table_id, table_id, key);
                    self.resolve_set(table_id, table_id, key);
                }
                TableResolution::NotFound { .. } | TableResolution::Ambiguous { .. } => {
                    self.emit(entity, field, None, SelectionTarget::Unresolved);
                    self.emit_unresolved(key);
                }
            }
        }
    }

    /// Resolves the children of `parent` against `table`, recursing into
    /// relations.
    fn resolve_set(&mut self, root: TableId, table: TableId, parent: NodeKey) {
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, key, _)| (*entity, *field, *key))
            .collect();
        for (entity, field, key) in fields {
            let reference = FieldRef {
                target: TableRef::parse(&field.name),
                selector: field.relation_path.as_deref(),
            };
            match self.catalog.check_field_ref(table, reference) {
                FieldCheckResult::Column(column) => {
                    let column_id = column.id;
                    self.emit(
                        entity,
                        field,
                        Some(table),
                        SelectionTarget::Column(column_id),
                    );
                    // Scalars have no legal clauses or children, but error
                    // recovery can produce them; they resolve to nothing.
                    self.emit_unresolved(key);
                }
                FieldCheckResult::Relation(relation) => {
                    let relation_table = relation.table.id;
                    let target = SelectionTarget::Relation {
                        table: relation_table,
                        foreign_key: relation.foreign_key.id,
                        selector: relation.selector.clone(),
                    };
                    self.emit(entity, field, Some(table), target);
                    self.resolve_clauses(root, relation_table, key);
                    self.resolve_set(root, relation_table, key);
                }
                FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {
                    self.emit(entity, field, Some(table), SelectionTarget::Unresolved);
                    self.emit_unresolved(key);
                }
            }
        }
    }

    fn resolve_clauses(&mut self, root: TableId, table: TableId, parent: NodeKey) {
        self.emit_clauses(parent, Some(ClauseContext { root, table }));
    }

    fn emit_clauses(&mut self, parent: NodeKey, context: Option<ClauseContext>) {
        let clauses: Vec<Entity> = self
            .tree
            .clauses_under(parent)
            .map(|(entity, _, _, _)| *entity)
            .collect();
        for clause in clauses {
            self.commands.insert((
                DerivedFrom::many([clause, self.catalog_entity]),
                BelongsToFile(self.file),
                ResolvedClause { clause, context },
            ));
        }
    }

    /// Emits unresolved facts for everything under an unresolvable parent.
    fn emit_unresolved(&mut self, parent: NodeKey) {
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, key, _)| (*entity, *field, *key))
            .collect();
        for (entity, field, key) in fields {
            self.emit(entity, field, None, SelectionTarget::Unresolved);
            self.emit_unresolved(key);
        }
        self.emit_clauses(parent, None);
    }
}
