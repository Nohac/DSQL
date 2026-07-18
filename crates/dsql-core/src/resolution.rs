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

use std::collections::{HashMap, HashSet};

use crate::schema::dsql_schema;
use bowl::{Commands, Component, DerivedFrom, Entity, In, Query, Registrar, Where};

use crate::catalog::{
    CatalogSnapshot, ColumnId, DataType, FieldCheckResult, FieldRef, ForeignKeyId,
    RelationCardinality, TableId, TableKey, TableRef, TableResolution,
};
use crate::entities::aggregate::{
    AggregateFunction, AggregateMode, AggregateProblem, AggregateProblemKind,
    resolve_aggregate_value,
};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::expression::{BinaryOp, Expr, LiteralValue, PathAnchor, PathSegment, Sigil};
use crate::entities::field_selection::{FieldSel, SelectionLimitSyntax};
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
    /// The authoritative row shape for table and relation selections.
    pub shape: Option<ResolvedSelectionShape>,
}

/// The semantic row cardinality of a resolved table or relation selection.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum SelectionCardinality {
    /// Zero or more rows, represented as an array.
    Collection,
    /// Zero or one row, represented as an object or `null`.
    AtMostOne,
}

/// The proof that made a selection at-most-one.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SelectionCardinalityProof {
    /// Catalog relationship metadata proves the edge singular.
    Relation,
    /// Mandatory equality predicates cover this catalog unique key.
    UniqueKey(Vec<ColumnId>),
    /// The selection has the compile-time literal `limit 1`.
    LimitOne,
}

/// The effective limit used by cardinality and limit diagnostics.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ResolvedSelectionLimit {
    /// No valid shape-relevant limit was written.
    None,
    /// A non-negative integer literal.
    Literal { value: u64, span: Span },
    /// A required runtime variable.
    Runtime { span: Span },
}

impl ResolvedSelectionLimit {
    /// Whether this limit may suppress an otherwise available singular row.
    pub fn may_suppress(self) -> bool {
        matches!(self, Self::Literal { value: 0, .. } | Self::Runtime { .. })
    }
}

/// Cardinality, nullability, proof, and limit semantics shared by checks,
/// aggregate resolution, planning, SQL, metadata, and generated types.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedSelectionShape {
    /// The public row container shape.
    pub cardinality: SelectionCardinality,
    /// The winning at-most-one proof, when one applies.
    pub proof: Option<SelectionCardinalityProof>,
    /// Whether an at-most-one row may be absent.
    pub nullable: bool,
    /// The effective written limit.
    pub limit: ResolvedSelectionLimit,
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
    /// Every scalar relation aggregate in expression order, keyed by span.
    pub aggregates: Vec<ResolvedPredicateAggregate>,
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

    /// The scalar predicate aggregate at `span`.
    pub fn aggregate_at(&self, span: Span) -> Option<&ResolvedPredicateAggregate> {
        self.aggregates
            .iter()
            .find(|aggregate| aggregate.span == span)
    }
}

/// One direct relation aggregate resolved inside a clause expression.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedPredicateAggregate {
    /// Span of the complete scalar aggregate expression.
    pub span: Span,
    /// Contextual function name span.
    pub function_span: Span,
    /// Terminal operand name, when an operand was written.
    pub operand_name_span: Option<Span>,
    /// Direct relation source, when it resolved.
    pub relation: Option<ResolvedRelationStep>,
    /// Aggregate function, when recognized.
    pub function: Option<AggregateFunction>,
    /// Direct related-table operand, when one resolved.
    pub operand: Option<ColumnId>,
    /// Logical scalar result type.
    pub data_type: Option<DataType>,
    /// Whether the aggregate can be SQL `NULL` on this source shape.
    pub nullable: bool,
    /// Stable typed failures found while resolving the expression.
    pub problems: Vec<AggregateProblem>,
}

impl ResolvedPredicateAggregate {
    /// Whether planning can safely consume this aggregate value.
    pub fn is_valid(&self) -> bool {
        self.problems.is_empty()
            && self.relation.is_some()
            && self.function.is_some()
            && self.data_type.is_some()
    }

    /// Stable inferred variable-path parts for this scalar value.
    pub fn display_path(&self, catalog: &crate::catalog::Catalog) -> Option<Vec<String>> {
        let relation = self.relation.as_ref()?;
        let function = self.function?;
        let mut parts = vec![relation.display.clone(), function.label().to_string()];
        if let Some(operand) = self.operand
            && let Some(column) = catalog.column_by_id(operand)
        {
            parts.push(column.name.clone());
        }
        Some(parts)
    }
}

/// Indexes clause resolutions by the clause entity they resolve.
///
/// Spans are only unique within one file, while fragment expansion walks
/// clauses across files. Consumers use this entity-keyed index to keep the
/// semantic fact paired with its owning clause without re-resolving names.
pub(crate) fn index_resolved_clauses<'a>(
    resolutions: impl IntoIterator<Item = &'a ResolvedClause>,
) -> HashMap<Entity, &'a ResolvedClause> {
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
    catalog: &crate::catalog::Catalog,
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
    let shape = resolve_selection_shape(catalog, selection, context, &target);
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
            shape,
        },
    ));
}

fn resolve_selection_shape(
    catalog: &crate::catalog::Catalog,
    selection: &FieldSel,
    context: Option<TableId>,
    target: &SelectionTarget,
) -> Option<ResolvedSelectionShape> {
    let table = target.child_context()?;
    let relation = match target {
        SelectionTarget::Relation { foreign_key, .. } => {
            context.zip(catalog.foreign_key_by_id(*foreign_key))
        }
        SelectionTarget::Table(_) => None,
        SelectionTarget::Column(_) | SelectionTarget::Unresolved => return None,
    };
    let relation_is_singular = relation.is_some_and(|(context, foreign_key)| {
        catalog.relation_cardinality(context, table, foreign_key)
            == Some(RelationCardinality::Singular)
    });
    let unique_key = selection
        .shape_syntax
        .predicate
        .as_ref()
        .and_then(|predicate| unique_key_for_predicate(catalog, table, predicate));
    let limit = match selection.shape_syntax.limit {
        SelectionLimitSyntax::None => ResolvedSelectionLimit::None,
        SelectionLimitSyntax::Literal { value, span } => {
            ResolvedSelectionLimit::Literal { value, span }
        }
        SelectionLimitSyntax::Runtime { span } => ResolvedSelectionLimit::Runtime { span },
    };
    let proof = if relation_is_singular {
        Some(SelectionCardinalityProof::Relation)
    } else if let Some(columns) = unique_key {
        Some(SelectionCardinalityProof::UniqueKey(columns))
    } else if matches!(limit, ResolvedSelectionLimit::Literal { value: 1, .. }) {
        Some(SelectionCardinalityProof::LimitOne)
    } else {
        None
    };
    let cardinality = if proof.is_some() {
        SelectionCardinality::AtMostOne
    } else {
        SelectionCardinality::Collection
    };
    let nullable = if cardinality == SelectionCardinality::AtMostOne {
        match target {
            SelectionTarget::Table(_) => true,
            SelectionTarget::Relation { .. } if !relation_is_singular => true,
            SelectionTarget::Relation { .. } => relation.is_none_or(|(context, foreign_key)| {
                catalog.relation_is_nullable(context, table, foreign_key)
                    || selection.shape_syntax.predicate.is_some()
                    || selection.shape_syntax.has_offset
                    || limit.may_suppress()
            }),
            SelectionTarget::Column(_) | SelectionTarget::Unresolved => false,
        }
    } else {
        false
    };

    Some(ResolvedSelectionShape {
        cardinality,
        proof,
        nullable,
        limit,
    })
}

fn unique_key_for_predicate(
    catalog: &crate::catalog::Catalog,
    table: TableId,
    predicate: &Expr,
) -> Option<Vec<ColumnId>> {
    let equalities = guaranteed_equalities(catalog, table, predicate);
    let table = catalog.table_by_id(table)?;
    if !table.primary_key.is_empty()
        && table
            .primary_key
            .iter()
            .all(|column| equalities.contains_key(column))
    {
        return Some(table.primary_key.clone());
    }
    table
        .unique_constraints
        .iter()
        .find(|columns| {
            !columns.is_empty() && columns.iter().all(|column| equalities.contains_key(column))
        })
        .cloned()
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum FixedValueIdentity {
    String(String),
    Number(String),
    Bool(bool),
    NamedVariable { sigil: Sigil, name: String },
    AnonymousVariable { sigil: Sigil, span: Span },
}

type GuaranteedEqualities = HashMap<ColumnId, HashSet<FixedValueIdentity>>;

fn guaranteed_equalities(
    catalog: &crate::catalog::Catalog,
    table: TableId,
    expr: &Expr,
) -> GuaranteedEqualities {
    let Expr::Binary { op, lhs, rhs, .. } = expr else {
        return GuaranteedEqualities::new();
    };
    match op {
        BinaryOp::And => {
            let mut equalities = guaranteed_equalities(catalog, table, lhs);
            for (column, identities) in guaranteed_equalities(catalog, table, rhs) {
                equalities.entry(column).or_default().extend(identities);
            }
            equalities
        }
        BinaryOp::Or => {
            let mut equalities = guaranteed_equalities(catalog, table, lhs);
            let right = guaranteed_equalities(catalog, table, rhs);
            equalities.retain(|column, identities| {
                let Some(right_identities) = right.get(column) else {
                    return false;
                };
                identities.retain(|identity| right_identities.contains(identity));
                !identities.is_empty()
            });
            equalities
        }
        BinaryOp::Comparison(crate::entities::expression::ComparisonOp::Eq) => {
            equality(catalog, table, lhs, rhs)
                .map(|(column, identity)| (column, HashSet::from([identity])))
                .into_iter()
                .collect()
        }
        BinaryOp::Variable(variable)
            if variable.operators.as_ref().is_some_and(|operators| {
                !operators.is_empty()
                    && operators
                        .iter()
                        .all(|operator| *operator == crate::entities::expression::ComparisonOp::Eq)
            }) =>
        {
            equality(catalog, table, lhs, rhs)
                .map(|(column, identity)| (column, HashSet::from([identity])))
                .into_iter()
                .collect()
        }
        BinaryOp::Comparison(_) | BinaryOp::Variable(_) => GuaranteedEqualities::new(),
    }
}

fn equality(
    catalog: &crate::catalog::Catalog,
    table: TableId,
    left: &Expr,
    right: &Expr,
) -> Option<(ColumnId, FixedValueIdentity)> {
    direct_current_column(catalog, table, left)
        .zip(fixed_value_identity(right))
        .or_else(|| direct_current_column(catalog, table, right).zip(fixed_value_identity(left)))
}

fn direct_current_column(
    catalog: &crate::catalog::Catalog,
    table: TableId,
    expr: &Expr,
) -> Option<ColumnId> {
    let Expr::Path {
        anchor: PathAnchor::Current,
        segments,
        ..
    } = expr
    else {
        return None;
    };
    let [segment] = segments.as_slice() else {
        return None;
    };
    if segment.relation_path.is_some() {
        return None;
    }
    match catalog.check_field_ref(
        table,
        FieldRef {
            target: TableRef::parse(&segment.name),
            selector: None,
        },
    ) {
        FieldCheckResult::Column(column) => Some(column.id),
        FieldCheckResult::Relation(_)
        | FieldCheckResult::NotFound
        | FieldCheckResult::AmbiguousRelation { .. } => None,
    }
}

fn fixed_value_identity(expr: &Expr) -> Option<FixedValueIdentity> {
    match expr {
        Expr::Variable { variable, .. } => match &variable.name {
            Some(name) => Some(FixedValueIdentity::NamedVariable {
                sigil: variable.sigil,
                name: name.clone(),
            }),
            None => Some(FixedValueIdentity::AnonymousVariable {
                sigil: variable.sigil,
                span: variable.span,
            }),
        },
        Expr::Literal {
            value: LiteralValue::String(value),
            ..
        } => Some(FixedValueIdentity::String(value.clone())),
        Expr::Literal {
            value: LiteralValue::Number(value),
            ..
        } => Some(FixedValueIdentity::Number(value.clone())),
        Expr::Literal {
            value: LiteralValue::Bool(value),
            ..
        } => Some(FixedValueIdentity::Bool(*value)),
        Expr::Literal {
            value: LiteralValue::Null,
            ..
        }
        | Expr::Path { .. }
        | Expr::Aggregate { .. }
        | Expr::Binary { .. }
        | Expr::Error { .. } => None,
    }
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
                catalog,
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
                catalog,
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
        catalog,
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
    let mut aggregates = Vec::new();
    let mut order_items = Vec::new();
    if let Some(context) = context {
        match clause {
            ClauseFact::Where { expr } => {
                collect_clause_values(catalog, context, expr, &mut paths, &mut aggregates)
            }
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
                collect_clause_values(catalog, context, expr, &mut paths, &mut aggregates);
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
            aggregates,
            order_items,
        },
    ));
}

/// Collects every `Expr::Path` of one clause expression, resolved in
/// expression order.
fn collect_clause_values(
    catalog: &crate::catalog::Catalog,
    context: ClauseContext,
    expr: &Expr,
    paths: &mut Vec<ResolvedPath>,
    aggregates: &mut Vec<ResolvedPredicateAggregate>,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            collect_clause_values(catalog, context, lhs, paths, aggregates);
            collect_clause_values(catalog, context, rhs, paths, aggregates);
        }
        Expr::Path {
            anchor, segments, ..
        } => paths.push(resolve_path(catalog, context, expr, anchor, segments)),
        Expr::Aggregate { .. } => {
            aggregates.push(resolve_predicate_aggregate(catalog, context, expr));
        }
        Expr::Literal { .. } | Expr::Variable { .. } | Expr::Error { .. } => {}
    }
}

fn resolve_predicate_aggregate(
    catalog: &crate::catalog::Catalog,
    context: ClauseContext,
    expr: &Expr,
) -> ResolvedPredicateAggregate {
    let Expr::Aggregate {
        source,
        function,
        function_span,
        operand,
        span,
    } = expr
    else {
        return ResolvedPredicateAggregate {
            span: expr.span(),
            function_span: expr.span(),
            operand_name_span: None,
            relation: None,
            function: None,
            operand: None,
            data_type: None,
            nullable: false,
            problems: Vec::new(),
        };
    };
    let mut problems = Vec::new();
    let relation = resolve_predicate_relation(catalog, context, source, &mut problems);
    let value = relation.as_ref().map(|relation| {
        resolve_aggregate_value(
            catalog,
            relation.table,
            AggregateMode::Ungrouped,
            function,
            *function_span,
            operand.as_deref(),
            *span,
            &mut problems,
        )
    });
    ResolvedPredicateAggregate {
        span: *span,
        function_span: *function_span,
        operand_name_span: value.as_ref().and_then(|value| value.operand_name_span),
        relation,
        function: value.as_ref().and_then(|value| value.function),
        operand: value.as_ref().and_then(|value| value.operand),
        data_type: value.as_ref().and_then(|value| value.data_type),
        nullable: value.is_some_and(|value| value.nullable),
        problems,
    }
}

fn resolve_predicate_relation(
    catalog: &crate::catalog::Catalog,
    context: ClauseContext,
    source: &Expr,
    problems: &mut Vec<AggregateProblem>,
) -> Option<ResolvedRelationStep> {
    let Expr::Path {
        anchor: PathAnchor::Current,
        segments,
        ..
    } = source
    else {
        push_invalid_predicate_source(source, problems);
        return None;
    };
    let [segment] = segments.as_slice() else {
        push_invalid_predicate_source(source, problems);
        return None;
    };
    let reference = FieldRef {
        target: TableRef::parse(&segment.name),
        selector: segment.relation_path.as_deref(),
    };
    let FieldCheckResult::Relation(relation) = catalog.check_field_ref(context.table, reference)
    else {
        push_invalid_predicate_source(source, problems);
        return None;
    };
    let collection = catalog
        .foreign_key_by_id(relation.foreign_key.id)
        .and_then(|foreign_key| {
            catalog.relation_cardinality(context.table, relation.table.id, foreign_key)
        })
        == Some(RelationCardinality::Collection);
    if !collection {
        push_invalid_predicate_source(source, problems);
    }
    Some(ResolvedRelationStep {
        span: segment.span,
        written: segment.name.clone(),
        display: reference.display_text(),
        foreign_key: relation.foreign_key.id,
        table: relation.table.id,
    })
}

fn push_invalid_predicate_source(source: &Expr, problems: &mut Vec<AggregateProblem>) {
    problems.push(AggregateProblem {
        span: source.span(),
        kind: AggregateProblemKind::PredicateSourceMustBeDirectCollection {
            source: source.to_string(),
        },
    });
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
