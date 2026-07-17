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

use crate::schema::dsql_schema;
use bowl::{Commands, Component, DerivedFrom, Entity, In, Query, Registrar, Where};

use crate::catalog::{
    CatalogSnapshot, ColumnId, FieldCheckResult, FieldRef, ForeignKeyId, TableId, TableKey,
    TableRef, TableResolution,
};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::expression::{Expr, PathAnchor, PathSegment};
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

/// The catalog resolution of a fragment declaration's `on` target. Checks
/// and services consume this shared fact instead of resolving the raw name
/// independently.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedFragmentTarget {
    /// Span of the target name in the fragment document.
    pub span: Span,
    /// The resolved table or the stable failure details used by diagnostics.
    pub target: ResolvedTableTarget,
}

/// Owned form of [`TableResolution`] suitable for a derived component.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ResolvedTableTarget {
    /// The target resolved to this catalog table.
    Table(TableId),
    /// No table matched the written reference.
    NotFound { reference: String },
    /// More than one table matched the written reference.
    Ambiguous {
        reference: String,
        candidates: Vec<TableKey>,
    },
}

impl SelectionTarget {
    /// The table this selection's own children and clauses resolve
    /// against, if any.
    pub(crate) fn child_context(&self) -> Option<TableId> {
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

/// The tables one clause's expressions resolve against — plus every
/// predicate path and order item resolved once, so checks, variable
/// inference, planning, lints, and highlighting share one semantic
/// decision instead of re-deriving it from raw strings. `context` is
/// `None` when the owning selection never resolved (nothing else
/// resolves either).
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedClause {
    /// The clause entity this resolution is about.
    pub clause: Entity,
    pub context: Option<ClauseContext>,
    /// Every `Expr::Path` in the clause, in expression order, keyed by
    /// its span.
    pub paths: Vec<ResolvedPath>,
    /// Order-by items, in written order.
    pub order_items: Vec<ResolvedOrderItem>,
}

impl ResolvedClause {
    /// The resolution of the path expression at `span`.
    pub fn path_at(&self, span: Span) -> Option<&ResolvedPath> {
        self.paths.iter().find(|path| path.span == span)
    }

    /// The resolution of the order item at `span`.
    pub fn order_item_at(&self, span: Span) -> Option<&ResolvedOrderItem> {
        self.order_items.iter().find(|item| item.span == span)
    }
}

/// Indexes clause resolutions by the clause entity they resolve.
///
/// Spans are only unique within one file, while fragment expansion walks
/// clauses across files. Consumers use this entity-keyed index to keep the
/// semantic fact paired with its owning clause without re-resolving names.
pub(crate) fn index_resolved_clauses<'a>(
    resolutions: impl IntoIterator<Item = &'a ResolvedClause>,
) -> std::collections::HashMap<Entity, &'a ResolvedClause> {
    resolutions
        .into_iter()
        .map(|resolution| (resolution.clause, resolution))
        .collect()
}

/// One predicate path resolved segment by segment against its clause
/// context.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Span of the whole path expression — the correlation key for
    /// consumers still walking the expression tree.
    pub span: Span,
    /// Where the path anchors (`.`, `~`, or `..`).
    pub anchor: PathAnchor,
    /// The path as written, for diagnostics.
    pub written: String,
    /// Relation steps that resolved, in path order; resolution stops at
    /// the first failure.
    pub relations: Vec<ResolvedRelationStep>,
    pub terminal: PathTerminal,
}

impl ResolvedPath {
    /// The display parts of a fully resolved column path.
    pub fn display_path(&self) -> Option<impl Iterator<Item = &str>> {
        let PathTerminal::Column { display, .. } = &self.terminal else {
            return None;
        };
        Some(
            self.relations
                .iter()
                .map(|step| step.display.as_str())
                .chain(std::iter::once(display.as_str())),
        )
    }
}

/// One resolved relation step of a predicate path.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedRelationStep {
    pub span: Span,
    /// The qualified name as written (selector excluded), aligned with
    /// `span` for span-splitting consumers.
    pub written: String,
    /// The reference's display form (selector included), for messages.
    pub display: String,
    pub foreign_key: ForeignKeyId,
    /// The table the step lands on.
    pub table: TableId,
}

/// Where a resolved path ends.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum PathTerminal {
    /// The full path resolved to a column of `table`.
    Column {
        span: Span,
        /// The terminal as written, aligned with `span`.
        written: String,
        /// The reference's display form (selector included), for messages.
        display: String,
        table: TableId,
        column: ColumnId,
    },
    /// Resolution failed (at a relation step or the terminal); the whole
    /// path diagnoses against the clause context.
    Failed,
    /// Parent-scope anchors are not resolvable at check time.
    OutOfScope,
}

impl PathTerminal {
    /// The terminal column, when the path fully resolved.
    pub fn column(&self) -> Option<ColumnId> {
        match self {
            PathTerminal::Column { column, .. } => Some(*column),
            PathTerminal::Failed | PathTerminal::OutOfScope => None,
        }
    }
}

/// One order-by item resolved against the clause's table.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedOrderItem {
    pub span: Span,
    /// The terminal column, when the item names one.
    pub column: Option<ColumnId>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ClauseContext {
    /// The query root's table (`~` paths anchor here).
    pub root: TableId,
    /// The clause's own table (`.` paths anchor here).
    pub table: TableId,
}

pub fn register_resolution(reg: &mut Registrar<'_>) {
    reg.system(resolve_fragment_targets);
    reg.system(resolve_roots);
    reg.system(resolve_nested);
    reg.system(resolve_clauses);
}

/// Resolves each fragment declaration target once against the catalog.
async fn resolve_fragment_targets(
    fragments: Query<(Entity, &DefDecl, &FragmentTarget, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::ResolvedFragmentTarget,)>,
) {
    let (fragment, _, target, file) = fragments.item();
    let (catalog_entity, snapshot) = catalog.item();
    let target_resolution = match snapshot
        .catalog()
        .resolve_table_ref_for(TableRef::parse(&target.name))
    {
        TableResolution::Found(table) => ResolvedTableTarget::Table(table.id),
        TableResolution::NotFound { reference } => ResolvedTableTarget::NotFound { reference },
        TableResolution::Ambiguous {
            reference,
            candidates,
        } => ResolvedTableTarget::Ambiguous {
            reference,
            candidates,
        },
    };
    commands.insert((
        DerivedFrom::many([fragment, catalog_entity]),
        BelongsToFile(file.0),
        ResolvedFragmentTarget {
            span: target.span,
            target: target_resolution,
        },
    ));
}

/// Emits one resolution fact.
#[expect(clippy::too_many_arguments, reason = "one emission site, all context")]
fn emit(
    commands: &mut Commands<(dsql_schema::ResolvedSelection,)>,
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
    mut commands: Commands<(dsql_schema::ResolvedSelection,)>,
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
    mut commands: Commands<(dsql_schema::ResolvedSelection,)>,
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
/// tracked invocation per (field, field-resolution, clause) triple. The
/// clause's predicate paths and order items resolve here, once — every
/// downstream stage consumes these facts instead of re-resolving raw
/// strings against the catalog.
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
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::ResolvedClause,)>,
) {
    let (field_entity, _, _, _, file) = parents.item();
    let (_, parent_resolved) = resolution.item();
    let (clause_entity, clause) = clauses.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let context = match (parent_resolved.root, parent_resolved.target.child_context()) {
        (Some(root), Some(table)) => Some(ClauseContext { root, table }),
        _ => None,
    };

    let mut paths = Vec::new();
    let mut order_items = Vec::new();
    if let Some(context) = context {
        match clause {
            ClauseFact::Where { expr } => collect_paths(catalog, context, expr, &mut paths),
            ClauseFact::OrderBy { items } => {
                order_items.extend(items.iter().map(|item| {
                    let reference = FieldRef {
                        target: TableRef::parse(&item.field),
                        selector: None,
                    };
                    let column = match catalog.check_field_ref(context.table, reference) {
                        FieldCheckResult::Column(column) => Some(column.id),
                        _ => None,
                    };
                    ResolvedOrderItem {
                        span: item.field_span,
                        column,
                    }
                }));
            }
            ClauseFact::Limit { expr } | ClauseFact::Offset { expr } => {
                collect_paths(catalog, context, expr, &mut paths);
            }
        }
    }

    commands.insert((
        DerivedFrom::many([field_entity, clause_entity, catalog_entity]),
        BelongsToFile(file.0),
        ResolvedClause {
            clause: clause_entity,
            context,
            paths,
            order_items,
        },
    ));
}

/// Collects every `Expr::Path` of one clause expression, resolved in
/// expression order.
fn collect_paths(
    catalog: &crate::catalog::Catalog,
    context: ClauseContext,
    expr: &Expr,
    paths: &mut Vec<ResolvedPath>,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            collect_paths(catalog, context, lhs, paths);
            collect_paths(catalog, context, rhs, paths);
        }
        Expr::Path {
            anchor, segments, ..
        } => paths.push(resolve_path(catalog, context, expr, anchor, segments)),
        Expr::Literal { .. } | Expr::Variable { .. } | Expr::Error { .. } => {}
    }
}

/// One path against the clause context: relation steps first, then the
/// terminal column, stopping at the first failure.
fn resolve_path(
    catalog: &crate::catalog::Catalog,
    context: ClauseContext,
    expr: &Expr,
    anchor: &PathAnchor,
    segments: &[PathSegment],
) -> ResolvedPath {
    let span = expr.span();
    let written = expr.to_string();

    let mut current = match anchor {
        PathAnchor::Current => context.table,
        PathAnchor::Root => context.root,
        PathAnchor::Parent => {
            return ResolvedPath {
                span,
                anchor: *anchor,
                written,
                relations: Vec::new(),
                terminal: PathTerminal::OutOfScope,
            };
        }
    };

    let mut relations = Vec::new();
    let Some((last, steps)) = segments.split_last() else {
        return ResolvedPath {
            span,
            anchor: *anchor,
            written,
            relations,
            terminal: PathTerminal::Failed,
        };
    };
    for segment in steps {
        let reference = FieldRef {
            target: TableRef::parse(&segment.name),
            selector: segment.relation_path.as_deref(),
        };
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(current, reference)
        else {
            return ResolvedPath {
                span,
                anchor: *anchor,
                written,
                relations,
                terminal: PathTerminal::Failed,
            };
        };
        relations.push(ResolvedRelationStep {
            span: segment.span,
            written: segment.name.clone(),
            display: reference.display_text(),
            foreign_key: relation.foreign_key.id,
            table: relation.table.id,
        });
        current = relation.table.id;
    }

    let reference = FieldRef {
        target: TableRef::parse(&last.name),
        selector: last.relation_path.as_deref(),
    };
    let terminal = match catalog.check_field_ref(current, reference) {
        FieldCheckResult::Column(column) => PathTerminal::Column {
            span: last.span,
            written: last.name.clone(),
            display: reference.display_text(),
            table: current,
            column: column.id,
        },
        _ => PathTerminal::Failed,
    };
    ResolvedPath {
        span,
        anchor: *anchor,
        written,
        relations,
        terminal,
    }
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
