//! Field-selection entity: one selected field or nested relation inside a
//! selection set, with its alias and relation-path selector — plus the
//! catalog check walk that validates every selection tree top-down.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query, SystemExt, SystemParam, View, With};

use crate::catalog::{CatalogSnapshot, FieldCheckResult, FieldRef, TableRef, TableResolution};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::fragment_spread::{SpreadDecl, check_spread_site};
use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage};
use crate::format::CstFormatter;
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    ParentKey, Severity, Span, emit_diagnostic,
};
use crate::service::completion::{CompletionContext, CompletionRequest};
use crate::service::hover::{HoverCandidate, HoverEnriched, HoverFile, Position, priority};
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};

/// PostgreSQL truncates result aliases beyond this many bytes
/// (`NAMEDATALEN - 1`), which silently corrupts output keys.
const POSTGRES_RESULT_ALIAS_MAX_BYTES: usize = 63;

/// One field selection, lowered from `field_selection`. Together with
/// [`ParentKey`] these facts are the flat encoding of the selection tree;
/// sibling order is byte order of [`FieldSel::span`].
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct FieldSel {
    /// Output alias, when written as `alias: field`.
    pub alias: Option<String>,
    /// Span of the alias, when present.
    pub alias_span: Option<Span>,
    /// The selected field or relation name (full qualified-name text).
    pub name: String,
    /// Explicit relation column selector, when written as `name->column`.
    pub relation_path: Option<String>,
    /// Span of the selected name (the target, not the alias).
    pub name_span: Span,
    /// Span of the whole selection including its clauses and children.
    pub span: Span,
    /// Whether the selection has a nested selection set.
    pub nested: bool,
    /// Whether the selection has a clause list, even an empty one —
    /// scalar fields must not have clauses at all.
    pub has_clause_list: bool,
}

/// Owns `field_selection` (and consumes `field_selection_tail` and
/// `field_suffix` from it).
pub struct FieldSelection;

impl LanguageEntity for FieldSelection {
    const NAME: &'static str = "field_selection";

    async fn register(bowl: &Bowl) {
        bowl.add_system(check_selections).await;
    }
}

impl LowerStage for FieldSelection {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
        let Some(first_ref) = direct_rule(ctx.cst, node, Rule::RelationRef) else {
            // Error recovery consumed the name; parse diagnostics cover it.
            return;
        };
        let tail = direct_rule(ctx.cst, node, Rule::FieldSelectionTail);
        let tail_ref = tail.and_then(|tail| direct_rule(ctx.cst, tail, Rule::RelationRef));

        // `alias: target` puts the alias first and the target in the tail;
        // without an alias the first relation_ref is the target itself.
        let (alias, alias_span, target) = match tail_ref {
            Some(target) => {
                let alias_span = node_span(ctx.cst, first_ref);
                (
                    Some(text(ctx.source, alias_span).to_string()),
                    Some(alias_span),
                    target,
                )
            }
            None => (None, None, first_ref),
        };

        let Some(name_node) = direct_rule(ctx.cst, target, Rule::QualifiedName) else {
            return;
        };
        let name_span = node_span(ctx.cst, name_node);
        // The `->column` selector Name is a direct child of relation_ref;
        // the relation name's own tokens sit nested inside qualified_name.
        let relation_path =
            direct_token(ctx.cst, target, Token::Name).map(|span| text(ctx.source, span).to_string());

        let suffix = tail.and_then(|tail| direct_rule(ctx.cst, tail, Rule::FieldSuffix));
        let nested = suffix
            .map(|suffix| direct_rule(ctx.cst, suffix, Rule::SelectionSet).is_some())
            .unwrap_or(false);
        let has_clause_list = suffix
            .map(|suffix| direct_rule(ctx.cst, suffix, Rule::ClauseList).is_some())
            .unwrap_or(false);

        let selection = FieldSel {
            alias,
            alias_span,
            name: text(ctx.source, name_span).to_string(),
            relation_path,
            name_span,
            span: node_span(ctx.cst, node),
            nested,
            has_clause_list,
        };

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        match ctx.parent {
            Some(parent) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                ParentKey(parent),
                selection,
            )),
            // Grammar-wise unreachable (selections only appear inside
            // definitions), but error recovery may orphan one; lower it
            // without a tree position rather than dropping the fact.
            None => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                selection,
            )),
        };
    }
}


/// Everything the check walk sees of one definition's file, gathered from
/// the ambient views and shared with the spread checks.
pub(crate) struct SelectionTree<'a> {
    pub(crate) fields: Vec<(Entity, &'a FieldSel, NodeKey, NodeKey)>,
    pub(crate) spreads: Vec<(Entity, &'a SpreadDecl, NodeKey, NodeKey)>,
    pub(crate) fragments: Vec<(Entity, &'a DefDecl, &'a FragmentTarget, NodeKey)>,
    pub(crate) clauses: Vec<(Entity, &'a ClauseFact, Span, NodeKey)>,
}

impl SelectionTree<'_> {
    pub(crate) fn fields_under(
        &self,
        parent: NodeKey,
    ) -> impl Iterator<Item = &(Entity, &FieldSel, NodeKey, NodeKey)> {
        self.fields.iter().filter(move |(_, _, _, p)| *p == parent)
    }

    pub(crate) fn spreads_under(
        &self,
        parent: NodeKey,
    ) -> impl Iterator<Item = &(Entity, &SpreadDecl, NodeKey, NodeKey)> {
        self.spreads.iter().filter(move |(_, _, _, p)| *p == parent)
    }

    pub(crate) fn fragment_named(
        &self,
        name: &str,
    ) -> Option<&(Entity, &DefDecl, &FragmentTarget, NodeKey)> {
        self.fragments
            .iter()
            .find(|(_, decl, _, _)| decl.name == name)
    }

    /// Gathers one file's lowered selection facts out of the ambient views.
    pub(crate) fn collect<'a>(views: &'a TreeViews<'_>, file: Entity) -> SelectionTree<'a> {
        SelectionTree {
            fields: views
                .fields
                .iter()
                .filter(|(_, _, key, _)| key.file == file)
                .map(|(entity, field, key, parent)| (entity, field, *key, parent.0))
                .collect(),
            spreads: views
                .spreads
                .iter()
                .filter(|(_, _, key, _)| key.file == file)
                .map(|(entity, spread, key, parent)| (entity, spread, *key, parent.0))
                .collect(),
            fragments: views
                .fragments
                .iter()
                .filter(|(_, _, _, _, fragment_file)| fragment_file.0 == file)
                .map(|(entity, decl, target, key, _)| (entity, decl, target, *key))
                .collect(),
            clauses: views
                .clauses
                .iter()
                .map(|(entity, clause, span, parent)| (entity, clause, *span, parent.0))
                .collect(),
        }
    }

    pub(crate) fn clauses_under(
        &self,
        parent: NodeKey,
    ) -> impl Iterator<Item = &(Entity, &ClauseFact, Span, NodeKey)> {
        self.clauses.iter().filter(move |(_, _, _, p)| *p == parent)
    }
}

/// Validates one definition's selection tree against the catalog, top-down:
/// query roots resolve as tables, nested selections as columns or relations
/// of their context table, spreads by target compatibility. Runs per
/// definition; the catalog query is a tracked input, so a schema change
/// reruns every definition. Demand-gated like every check.
///
/// The per-construct logic stays with its owning entity: spread sites are
/// checked by [`check_spread_site`] in `fragment_spread`.
/// The ambient views the check and inference walks read, bundled to keep
/// system signatures within porridge's parameter arity.
#[derive(SystemParam)]
pub(crate) struct TreeViews<'a> {
    fields: View<'a, (Entity, &'a FieldSel, &'a NodeKey, &'a ParentKey)>,
    spreads: View<'a, (Entity, &'a SpreadDecl, &'a NodeKey, &'a ParentKey)>,
    fragments: View<'a, (Entity, &'a DefDecl, &'a FragmentTarget, &'a NodeKey, &'a BelongsToFile)>,
    clauses: View<'a, (Entity, &'a ClauseFact, &'a Span, &'a ParentKey)>,
}

async fn check_selections(
    _: Query<Entity, With<DiagnosticsDemand>>,
    defs: Query<(Entity, &DefDecl, &NodeKey, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (def_entity, decl, def_key, file) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let tree = SelectionTree::collect(&views, file.0);

    let mut ctx = CheckCtx {
        tree: &tree,
        catalog,
        catalog_entity,
        file: file.0,
        commands: &mut commands,
    };

    match decl.kind {
        DefKind::Query => ctx.check_query_roots(*def_key),
        DefKind::Fragment => ctx.check_fragment_body(def_entity, *def_key),
    }
}

/// Shared state of one definition's check walk.
pub(crate) struct CheckCtx<'a, 'view> {
    pub(crate) tree: &'a SelectionTree<'view>,
    pub(crate) catalog: &'a crate::catalog::Catalog,
    pub(crate) catalog_entity: Entity,
    pub(crate) file: Entity,
    pub(crate) commands: &'a mut Commands,
}

impl CheckCtx<'_, '_> {
    pub(crate) fn error(
        &mut self,
        anchor: Entity,
        span: Span,
        code: DiagnosticCode,
        message: String,
    ) {
        emit_diagnostic(
            self.commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([anchor, self.catalog_entity]),
                file: self.file,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code,
                message,
            },
        );
    }

    /// Query roots name tables; everything below them is field context.
    fn check_query_roots(&mut self, def_key: NodeKey) {
        self.check_output_keys(def_key);
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
                    let root_clauses: Vec<_> = self
                        .tree
                        .clauses_under(key)
                        .map(|(entity, clause, span, _)| (*entity, *clause, *span))
                        .collect();
                    for (clause_entity, clause, clause_span) in root_clauses {
                        crate::entities::clause::check_clause(
                            self,
                            table_id,
                            table_id,
                            clause_entity,
                            clause,
                            clause_span,
                        );
                    }
                    if !field.nested {
                        self.error(
                            entity,
                            field.name_span,
                            DiagnosticCode::RelationSelectionSet,
                            format!("relation field `{}` must have a selection set", field.name),
                        );
                        continue;
                    }
                    self.check_set(table_id, table_id, key);
                }
                TableResolution::NotFound { reference } => {
                    self.error(
                        entity,
                        field.name_span,
                        DiagnosticCode::TableNotFound,
                        format!("table `{reference}` not found"),
                    );
                }
                TableResolution::Ambiguous { reference, candidates } => {
                    let candidates: Vec<String> = candidates
                        .iter()
                        .map(|key| format!("{}::{}", key.schema, key.table))
                        .collect();
                    self.error(
                        entity,
                        field.name_span,
                        DiagnosticCode::AmbiguousTable,
                        format!(
                            "table `{reference}` is ambiguous; use an alias with a schema-qualified name ({})",
                            candidates.join(", ")
                        ),
                    );
                }
            }
        }
    }

    /// Fragment bodies check against the fragment's declared target. An
    /// unresolvable target is reported by the definition entity's own
    /// check; the body is skipped rather than double-reported.
    fn check_fragment_body(&mut self, def_entity: Entity, def_key: NodeKey) {
        let Some((_, _, target, _)) = self
            .tree
            .fragments
            .iter()
            .find(|(entity, _, _, _)| *entity == def_entity)
        else {
            return;
        };
        let Some(table) = self.catalog.table_ref_for(TableRef::parse(&target.name)) else {
            return;
        };
        let table_id = table.id;
        self.check_set(table_id, table_id, def_key);
    }

    /// Checks one selection set (the children of `parent`) against its
    /// context table, then recurses into relation selections.
    pub(crate) fn check_set(
        &mut self,
        root_table: crate::catalog::TableId,
        table: crate::catalog::TableId,
        parent: NodeKey,
    ) {
        self.check_output_keys(parent);

        let table_name = match self.catalog.table_by_id(table) {
            Some(table) => table.name.clone(),
            None => return,
        };

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
                    let data_type = column.data_type;
                    if field.nested {
                        self.error(
                            entity,
                            field.name_span,
                            DiagnosticCode::ScalarSelectionSet,
                            format!(
                                "field `{}` is a scalar ({}) and cannot have a selection set",
                                field.name,
                                data_type.as_str()
                            ),
                        );
                    }
                    if field.has_clause_list {
                        self.error(
                            entity,
                            field.name_span,
                            DiagnosticCode::ScalarClauses,
                            format!(
                                "field `{}` is a scalar ({}); only relations can have clauses",
                                field.name,
                                data_type.as_str()
                            ),
                        );
                    }
                }
                FieldCheckResult::Relation(relation) => {
                    let relation_table = relation.table.id;
                    let field_clauses: Vec<_> = self
                        .tree
                        .clauses_under(key)
                        .map(|(entity, clause, span, _)| (*entity, *clause, *span))
                        .collect();
                    for (clause_entity, clause, clause_span) in field_clauses {
                        crate::entities::clause::check_clause(
                            self,
                            root_table,
                            relation_table,
                            clause_entity,
                            clause,
                            clause_span,
                        );
                    }
                    if !field.nested {
                        self.error(
                            entity,
                            field.name_span,
                            DiagnosticCode::RelationSelectionSet,
                            format!("relation field `{}` must have a selection set", field.name),
                        );
                        continue;
                    }
                    self.check_set(root_table, relation_table, key);
                }
                FieldCheckResult::NotFound => {
                    self.error(
                        entity,
                        field.name_span,
                        DiagnosticCode::FieldNotFound,
                        format!(
                            "field `{}` not found on table `{table_name}`",
                            reference.display_text()
                        ),
                    );
                }
                FieldCheckResult::AmbiguousRelation {
                    reference,
                    candidates,
                } => {
                    self.error(
                        entity,
                        field.name_span,
                        DiagnosticCode::AmbiguousRelation,
                        format!(
                            "relation `{reference}` has multiple foreign-key paths; use one of: {}",
                            candidates.join(", ")
                        ),
                    );
                }
            }
        }

        let spreads: Vec<_> = self
            .tree
            .spreads_under(parent)
            .map(|(entity, spread, key, _)| (*entity, *spread, *key))
            .collect();
        for (entity, spread, _) in spreads {
            check_spread_site(self, entity, spread, table);
        }
    }

    /// Output keys must be unique within one selection set and fit
    /// PostgreSQL's result-alias limit.
    fn check_output_keys(&mut self, parent: NodeKey) {
        let mut seen: Vec<String> = Vec::new();
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect();
        for (entity, field) in fields {
            let key = field
                .alias
                .clone()
                .unwrap_or_else(|| TableRef::parse(&field.name).name.to_string());
            if seen.contains(&key) {
                self.error(
                    entity,
                    field.name_span,
                    DiagnosticCode::DuplicateOutputKey,
                    format!("selection output key `{key}` is ambiguous; use an alias"),
                );
            } else {
                seen.push(key.clone());
            }
            let bytes = key.len();
            if bytes > POSTGRES_RESULT_ALIAS_MAX_BYTES {
                self.error(
                    entity,
                    field.alias_span.unwrap_or(field.name_span),
                    DiagnosticCode::OutputKeyTooLong,
                    format!(
                        "selection output key `{key}` is {bytes} bytes; PostgreSQL result aliases must be at most {POSTGRES_RESULT_ALIAS_MAX_BYTES} bytes"
                    ),
                );
            }
        }
    }
}


impl FormatStage for FieldSelection {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        let first = formatter.direct_relation_ref_text(node);
        let tail = formatter.direct_rule(node, Rule::FieldSelectionTail);
        let (alias, name, suffix) = if let Some(tail) = tail {
            let tail_name = formatter.direct_relation_ref_text(tail);
            if tail_name.is_some() {
                (
                    first,
                    tail_name,
                    formatter.direct_rule(tail, Rule::FieldSuffix),
                )
            } else {
                (None, first, formatter.direct_rule(tail, Rule::FieldSuffix))
            }
        } else {
            (None, first, None)
        };
        if let Some(alias) = alias {
            formatter.write_str(&alias);
            formatter.write_str(": ");
        }
        if let Some(name) = name {
            formatter.write_str(&name);
        }
        if let Some(suffix) = suffix {
            formatter.field_suffix(suffix);
        }
    }
}


impl HoverStage for FieldSelection {
    async fn register_hover(bowl: &Bowl) {
        bowl.add_system(hover_fields.run_during(bowl::Phase::Complete))
            .await;
    }
}

/// Answers hover on a field selection name with its resolved column or
/// relation, walking the parent chain to establish the context table.
async fn hover_fields(
    query: Query<(Entity, &HoverFile, &Position), With<HoverEnriched>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (request, file, position) = query.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let tree = SelectionTree::collect(&views, file.0);
    let Some((_, field, key, _)) = tree
        .fields
        .iter()
        .find(|(_, field, key, _)| {
            key.file == file.0
                && field.name_span.start <= position.offset
                && position.offset < field.name_span.end
        })
        .copied()
    else {
        return;
    };

    let Some(table) = resolve_context_table(&tree, catalog, key) else {
        return;
    };

    let text =
        describe_field(catalog, table, field).unwrap_or_else(|| format!("`{}`", field.name));

    commands.insert((
        DerivedFrom::new(request),
        HoverCandidate {
            request,
            priority: priority::FIELD,
            text,
        },
    ));
}

/// The table a field's *own* reference resolves against: its parent's
/// resolved relation table, or the root table for query roots.
pub(crate) fn resolve_context_table(
    tree: &SelectionTree<'_>,
    catalog: &crate::catalog::Catalog,
    field_key: NodeKey,
) -> Option<crate::catalog::TableId> {
    // Build the ancestor chain of field keys, root first.
    let mut chain = Vec::new();
    let mut current = field_key;
    loop {
        let (_, _, _, parent) = tree.fields.iter().find(|(_, _, key, _)| *key == current)?;
        chain.push(current);
        match tree.fields.iter().find(|(_, _, key, _)| key == parent) {
            Some(_) => current = *parent,
            // Parent is the definition node.
            None => break,
        }
    }
    let root_key = *chain.last()?;

    // Query roots resolve as tables; fragment bodies against the target.
    let (_, _, _, root_parent) = tree.fields.iter().find(|(_, _, key, _)| *key == root_key)?;
    let fragment_target = tree
        .fragments
        .iter()
        .find(|(_, _, _, def_key)| def_key == root_parent)
        .map(|(_, _, target, _)| target.name.clone());

    let (_, root_field, _, _) = tree.fields.iter().find(|(_, _, key, _)| *key == root_key)?;
    let mut table = match &fragment_target {
        Some(target) => catalog.table_ref_for(TableRef::parse(target))?.id,
        None => {
            let table = catalog.table_ref_for(TableRef::parse(&root_field.name))?.id;
            if chain.len() == 1 {
                // Hovering the root itself: its context is itself.
                return Some(table);
            }
            table
        }
    };

    // Descend from below the root to the hovered field's parent.
    for key in chain.iter().rev().skip(if fragment_target.is_some() { 0 } else { 1 }) {
        if *key == field_key {
            return Some(table);
        }
        let (_, field, _, _) = tree.fields.iter().find(|(_, _, k, _)| k == key)?;
        let reference = FieldRef {
            target: TableRef::parse(&field.name),
            selector: field.relation_path.as_deref(),
        };
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(table, reference)
        else {
            return None;
        };
        table = relation.table.id;
    }
    Some(table)
}

fn describe_field(
    catalog: &crate::catalog::Catalog,
    table: crate::catalog::TableId,
    field: &FieldSel,
) -> Option<String> {
    // Root selections name tables directly.
    if let Some(root_table) = catalog.table_ref_for(TableRef::parse(&field.name))
        && root_table.id == table
    {
        return Some(format!(
            "table `{}`.`{}`",
            root_table.schema, root_table.name
        ));
    }
    let reference = FieldRef {
        target: TableRef::parse(&field.name),
        selector: field.relation_path.as_deref(),
    };
    match catalog.check_field_ref(table, reference) {
        FieldCheckResult::Column(column) => Some(format!(
            "column `{}`: {}{}",
            column.name,
            column.data_type.as_str(),
            if column.not_null { " (not null)" } else { "" },
        )),
        FieldCheckResult::Relation(relation) => Some(format!(
            "relation `{}` → `{}`.`{}` via `{}`",
            field.name, relation.table.schema, relation.table.name, relation.selector,
        )),
        _ => None,
    }
}


/// The table a field selection *targets*: the table itself for query
/// roots, or the relation's table for nested selections. This is the
/// context for everything inside the field's braces and clauses.
pub(crate) fn resolve_field_target(
    tree: &SelectionTree<'_>,
    catalog: &crate::catalog::Catalog,
    field_key: NodeKey,
) -> Option<crate::catalog::TableId> {
    let (_, field, _, parent) = tree.fields.iter().find(|(_, _, key, _)| *key == field_key)?;
    let is_query_root = !tree.fields.iter().any(|(_, _, key, _)| key == parent)
        && !tree.fragments.iter().any(|(_, _, _, def_key)| def_key == parent);
    if is_query_root {
        return catalog
            .table_ref_for(TableRef::parse(&field.name))
            .map(|table| table.id);
    }

    // Nested and fragment-root fields alike: resolve the containing context
    // (resolve_context_table handles fragment targets), then step through
    // this field's own relation reference.
    let context = resolve_context_table(tree, catalog, field_key)?;
    let reference = FieldRef {
        target: TableRef::parse(&field.name),
        selector: field.relation_path.as_deref(),
    };
    match catalog.check_field_ref(context, reference) {
        FieldCheckResult::Relation(relation) => Some(relation.table.id),
        _ => None,
    }
}


impl CompletionStage for FieldSelection {
    async fn register_completions(bowl: &Bowl) {
        bowl.add_system(complete_selections.run_during(bowl::Phase::Complete))
            .await;
    }
}

/// Contributes tables at query roots and columns/relations inside
/// selection bodies, disambiguating multi-path relations with their
/// `->selector`.
async fn complete_selections(
    requests: Query<(Entity, &CompletionContext), With<CompletionRequest>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    use crate::service::completion::{CompletionCandidate, CompletionItem, CompletionKind, CompletionSite};

    let (request, context) = requests.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let mut push = |item: CompletionItem| {
        commands.insert((
            DerivedFrom::new(request),
            CompletionCandidate { request, item },
        ));
    };

    match (context.site, context.table) {
        (CompletionSite::RootSelection, _) => {
            for table in &catalog.tables {
                let label = if table.schema == catalog.default_schema() {
                    table.name.clone()
                } else {
                    format!("{}::{}", table.schema, table.name)
                };
                push(CompletionItem {
                    label,
                    kind: CompletionKind::Table,
                    detail: Some(format!("table {}.{}", table.schema, table.name)),
                    insert_text: None,
                });
            }
        }
        (CompletionSite::SelectionBody, Some(table)) => {
            for column in catalog.columns_for_table(table) {
                push(CompletionItem {
                    label: column.name.clone(),
                    kind: CompletionKind::Column,
                    detail: Some(column.data_type.as_str().to_string()),
                    insert_text: None,
                });
            }
            let relations = catalog.relation_fields_for_table(table);
            for relation in &relations {
                let shared_paths = relations
                    .iter()
                    .filter(|candidate| candidate.name == relation.name)
                    .count();
                let label = if shared_paths > 1 {
                    format!("{}->{}", relation.name, relation.selector)
                } else {
                    relation.name.to_string()
                };
                push(CompletionItem {
                    label,
                    kind: CompletionKind::Relation,
                    detail: Some(format!(
                        "relation to {}.{} via {}",
                        relation.table.schema, relation.table.name, relation.selector
                    )),
                    insert_text: None,
                });
            }
        }
        _ => {}
    }
}
