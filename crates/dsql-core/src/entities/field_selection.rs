//! Field-selection entity: one selected field or nested relation inside a
//! selection set, with its alias and relation-path selector — plus the
//! catalog check walk that validates every selection tree top-down.

use std::collections::HashMap;

use bowl::{
    Commands, Component, DerivedFrom, Entity, Query, Registrar, SystemExt, SystemParam, View, With,
};

use crate::catalog::{CatalogSnapshot, FieldCheckResult, FieldRef, TableRef, TableResolution};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::expansion::{ExpandedSpread, SpreadExpansion};
use crate::entities::fragment_spread::{SpreadDecl, check_spread_site};
use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{
    CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage,
};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::resolution::{ResolvedClause, ResolvedSelection};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{CompletionContext, CompletionRequest};
use crate::service::hover::{Cursor, HoverCandidate, HoverEnriched, RequestKey, priority};
use crate::source::{ResolutionScope, ScopeImports};

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

    fn register(reg: &mut Registrar<'_>) {
        // Views lowered facts ambiently: behind the Complete barrier.
        reg.system(check_selections.run_during(bowl::Phase::Complete));
    }
}

impl LowerStage for FieldSelection {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let Some(first_ref) = direct_rule(ctx.cst, node, Rule::RelationRef) else {
            // Error recovery consumed the name; parse diagnostics cover it.
            return None;
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

        let name_node = direct_rule(ctx.cst, target, Rule::QualifiedName)?;
        let name_span = node_span(ctx.cst, name_node);
        // The `->column` selector Name is a direct child of relation_ref;
        // the relation name's own tokens sit nested inside qualified_name.
        let relation_path = direct_token(ctx.cst, target, Token::Name)
            .map(|span| text(ctx.source, span).to_string());

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

        // A parentless selection is grammar-wise unreachable (selections
        // only appear inside definitions), but error recovery may orphan
        // one; it lowers without a tree position rather than dropping.
        let entity = match ctx.parent {
            Some(parent) => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    selection,
                    ChildOf(parent),
                ))
                .untyped(),
            None => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    selection,
                ))
                .untyped(),
        };
        Some(entity)
    }
}

/// Everything the check walk sees of one definition's file, gathered from
/// the ambient views and shared with the spread checks. Tree edges are the
/// engine-maintained [`ChildOf`] relationships; entities orphaned by error
/// recovery carry no edge and stay out of every walk.
pub(crate) struct SelectionTree<'a> {
    /// (entity, fact, CST node key, parent entity), indexed by parent.
    pub(crate) fields: HashMap<Entity, Vec<(Entity, &'a FieldSel, NodeKey, Entity)>>,
    /// The same field rows, indexed by their own entity.
    pub(crate) fields_by_entity: HashMap<Entity, (Entity, &'a FieldSel, NodeKey, Entity)>,
    pub(crate) spreads: HashMap<Entity, Vec<(Entity, &'a SpreadDecl, Entity)>>,
    pub(crate) fragments: Vec<(Entity, &'a DefDecl, &'a FragmentTarget, &'a ResolutionScope)>,
    pub(crate) clauses: HashMap<Entity, Vec<(Entity, &'a ClauseFact, Span, Entity)>>,
}

impl SelectionTree<'_> {
    pub(crate) fn fields_under(
        &self,
        parent: Entity,
    ) -> impl Iterator<Item = &(Entity, &FieldSel, NodeKey, Entity)> {
        self.fields.get(&parent).into_iter().flatten()
    }

    pub(crate) fn spreads_under(
        &self,
        parent: Entity,
    ) -> impl Iterator<Item = &(Entity, &SpreadDecl, Entity)> {
        self.spreads.get(&parent).into_iter().flatten()
    }

    /// Gathers the lowered selection facts out of the ambient views,
    /// indexed by parent entity so tree descent is a lookup, not a linear
    /// scan over every fact in the project. The tree spans every file:
    /// edges are entity links so they never cross files, while fragments
    /// resolve across files by scope.
    pub(crate) fn collect<'a>(views: &'a TreeViews<'_>) -> SelectionTree<'a> {
        let mut fields: HashMap<Entity, Vec<(Entity, &FieldSel, NodeKey, Entity)>> = HashMap::new();
        for (entity, field, key, parent) in views.fields.iter() {
            fields
                .entry(parent.0)
                .or_default()
                .push((entity, field, *key, parent.0));
        }
        let mut spreads: HashMap<Entity, Vec<(Entity, &SpreadDecl, Entity)>> = HashMap::new();
        for (entity, spread, parent) in views.spreads.iter() {
            spreads
                .entry(parent.0)
                .or_default()
                .push((entity, spread, parent.0));
        }
        let mut clauses: HashMap<Entity, Vec<(Entity, &ClauseFact, Span, Entity)>> = HashMap::new();
        for (entity, clause, span, parent) in views.clauses.iter() {
            clauses
                .entry(parent.0)
                .or_default()
                .push((entity, clause, *span, parent.0));
        }
        let fields_by_entity = fields.values().flatten().map(|row| (row.0, *row)).collect();
        SelectionTree {
            fields,
            fields_by_entity,
            spreads,
            fragments: views.fragments.iter().collect(),
            clauses,
        }
    }

    /// The uniquely visible fragment `name` from `scope`, per the effective
    /// resolver. Zero or several candidates resolve to `None`; the spread
    /// checks report those cases.
    pub(crate) fn resolve_fragment(
        &self,
        name: &str,
        scope: &str,
        imports: &ScopeImports,
    ) -> Option<&(Entity, &DefDecl, &FragmentTarget, &ResolutionScope)> {
        let mut candidates = self
            .fragments
            .iter()
            .filter(|(_, decl, _, fragment_scope)| {
                decl.kind == DefKind::Fragment
                    && decl.name == name
                    && imports
                        .visible_from(scope)
                        .any(|visible| visible == fragment_scope.0)
            });
        let first = candidates.next()?;
        candidates.next().is_none().then_some(first)
    }

    pub(crate) fn clauses_under(
        &self,
        parent: Entity,
    ) -> impl Iterator<Item = &(Entity, &ClauseFact, Span, Entity)> {
        self.clauses.get(&parent).into_iter().flatten()
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
    fields: View<'a, (Entity, &'a FieldSel, &'a NodeKey, &'a ChildOf)>,
    spreads: View<'a, (Entity, &'a SpreadDecl, &'a ChildOf)>,
    fragments: View<'a, (Entity, &'a DefDecl, &'a FragmentTarget, &'a ResolutionScope)>,
    clauses: View<'a, (Entity, &'a ClauseFact, &'a Span, &'a ChildOf)>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "system parameters are the tracked join, not an API surface"
)]
async fn check_selections(
    _: Query<Entity, With<DiagnosticsDemand>>,
    defs: Query<(Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    _index: Query<(Entity, &crate::entities::definition::DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    views: TreeViews<'_>,
    resolutions: View<'_, (Entity, &ResolvedClause)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (def_entity, decl, file, scope) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (_, imports) = imports.item();
    let catalog = snapshot.catalog();

    let tree = SelectionTree::collect(&views);
    let clause_resolutions: std::collections::HashMap<Entity, &ResolvedClause> = resolutions
        .iter()
        .map(|(_, resolved)| (resolved.clause, resolved))
        .collect();

    let mut ctx = CheckCtx {
        tree: &tree,
        clause_resolutions: &clause_resolutions,
        catalog,
        catalog_entity,
        file: file.0,
        scope: &scope.0,
        enclosing_fragment: None,
        commands: &mut commands,
        imports,
    };

    match decl.kind {
        DefKind::Query => ctx.check_query_roots(def_entity),
        DefKind::Fragment => ctx.check_fragment_body(def_entity),
    }
}

/// Shared state of one definition's check walk.
pub(crate) struct CheckCtx<'a, 'view> {
    pub(crate) tree: &'a SelectionTree<'view>,
    /// Clause resolutions by clause entity: the one place predicate paths
    /// and order items were resolved.
    pub(crate) clause_resolutions: &'a std::collections::HashMap<Entity, &'view ResolvedClause>,
    pub(crate) catalog: &'a crate::catalog::Catalog,
    pub(crate) catalog_entity: Entity,
    pub(crate) file: Entity,
    /// Resolution scope of the definition being checked.
    pub(crate) scope: &'a str,
    /// Name of the fragment whose body is being checked, when any: seeds
    /// spread expansion so self-spreads read as cycles, not duplicates.
    pub(crate) enclosing_fragment: Option<String>,
    pub(crate) imports: &'a ScopeImports,
    pub(crate) commands: &'a mut Commands<(dsql_schema::Diagnostic,)>,
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
    fn check_query_roots(&mut self, def_entity: Entity) {
        self.check_output_keys(def_entity);
        let roots: Vec<_> = self
            .tree
            .fields_under(def_entity)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect();
        for (entity, field) in roots {
            match self
                .catalog
                .resolve_table_ref_for(TableRef::parse(&field.name))
            {
                TableResolution::Found(table) => {
                    let table_id = table.id;
                    let root_clauses: Vec<_> = self
                        .tree
                        .clauses_under(entity)
                        .map(|(entity, clause, span, _)| (*entity, *clause, *span))
                        .collect();
                    for (clause_entity, clause, clause_span) in root_clauses {
                        crate::entities::clause::check_clause(
                            self,
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
                    self.check_set(table_id, entity);
                }
                TableResolution::NotFound { reference } => {
                    self.error(
                        entity,
                        field.name_span,
                        DiagnosticCode::TableNotFound,
                        format!("table `{reference}` not found"),
                    );
                }
                TableResolution::Ambiguous {
                    reference,
                    candidates,
                } => {
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
    fn check_fragment_body(&mut self, def_entity: Entity) {
        let Some((_, decl, target, _)) = self
            .tree
            .fragments
            .iter()
            .find(|(entity, _, _, _)| *entity == def_entity)
        else {
            return;
        };
        self.enclosing_fragment = Some(decl.name.clone());
        let Some(table) = self.catalog.table_ref_for(TableRef::parse(&target.name)) else {
            return;
        };
        let table_id = table.id;
        self.check_set(table_id, def_entity);
    }

    /// Checks one selection set (the children of `parent`) against its
    /// context table, then recurses into relation selections.
    pub(crate) fn check_set(&mut self, table: crate::catalog::TableId, parent: Entity) {
        self.check_output_keys(parent);

        let table_name = match self.catalog.table_by_id(table) {
            Some(table) => table.name.clone(),
            None => return,
        };

        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect();
        for (entity, field) in fields {
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
                        .clauses_under(entity)
                        .map(|(entity, clause, span, _)| (*entity, *clause, *span))
                        .collect();
                    for (clause_entity, clause, clause_span) in field_clauses {
                        crate::entities::clause::check_clause(
                            self,
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
                    self.check_set(relation_table, entity);
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
            .map(|(entity, spread, _)| (*entity, *spread))
            .collect();
        for (entity, spread) in spreads {
            check_spread_site(self, entity, spread, table);
        }
    }

    /// Output keys must be unique within one selection set and fit
    /// PostgreSQL's result-alias limit.
    fn check_output_keys(&mut self, parent: Entity) {
        let mut seen: Vec<String> = Vec::new();
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect();
        for (entity, field) in fields {
            let key = output_key(field);
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

        // Spreads splice their fragment's top-level keys into this set:
        // collisions with local fields (or other spreads) are just as
        // ambiguous as two local fields, and diagnose at the spread site.
        let spreads: Vec<_> = self
            .tree
            .spreads_under(parent)
            .map(|(entity, spread, _)| (*entity, spread.name.clone(), spread.name_span))
            .collect();
        let tree = self.tree;
        let mut expansion = SpreadExpansion::new(tree, self.scope, self.imports);
        if let Some(enclosing) = &self.enclosing_fragment {
            expansion.seed(enclosing);
        }
        for (entity, name, name_span) in spreads {
            let ExpandedSpread::Fragment {
                entity: fragment, ..
            } = expansion.enter(&name)
            else {
                continue;
            };
            let mut keys = Vec::new();
            collect_expanded_keys(tree, &mut expansion, fragment, &mut keys);
            expansion.leave();
            for key in keys {
                if seen.contains(&key) {
                    self.error(
                        entity,
                        name_span,
                        DiagnosticCode::DuplicateOutputKey,
                        format!(
                            "spread `{name}` introduces duplicate output key `{key}`; use an alias"
                        ),
                    );
                } else {
                    seen.push(key);
                }
            }
        }
    }
}

/// Output key of one field: alias, or the object name of its target.
fn output_key(field: &FieldSel) -> String {
    field
        .alias
        .clone()
        .unwrap_or_else(|| TableRef::parse(&field.name).name.to_string())
}

/// The top-level output keys a fragment splices into a spreading set,
/// following the fragment's own top-level spreads through the shared
/// expansion (cycles cut off).
fn collect_expanded_keys(
    tree: &SelectionTree<'_>,
    expansion: &mut SpreadExpansion<'_, '_>,
    fragment: Entity,
    keys: &mut Vec<String>,
) {
    for (_, field, _, _) in tree.fields_under(fragment) {
        keys.push(output_key(field));
    }
    let spreads: Vec<String> = tree
        .spreads_under(fragment)
        .map(|(_, spread, _)| spread.name.clone())
        .collect();
    for name in spreads {
        if let ExpandedSpread::Fragment { entity, .. } = expansion.enter(&name) {
            collect_expanded_keys(tree, expansion, entity, keys);
            expansion.leave();
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
    fn register_hover(reg: &mut Registrar<'_>) {
        reg.system(hover_fields);
    }
}

/// Answers hover on a field selection name with its resolved column or
/// relation: one tracked invocation per (request, field-in-file) pair via
/// the `BelongsToFile` join, the meaning read off the field's
/// [`ResolvedSelection`] stamp — no views, no walk, no phase barrier.
async fn hover_fields(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    fields: Query<(Entity, &ResolvedSelection), bowl::Where<bowl::Eq<BelongsToFile>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, resolved) = fields.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    if !(resolved.name_span.start <= cursor.0 && cursor.0 < resolved.name_span.end) {
        return;
    }

    let text = describe_target(catalog, resolved).unwrap_or_else(|| format!("`{}`", resolved.name));

    commands.insert((
        DerivedFrom::new(request),
        RequestKey(request),
        HoverCandidate {
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
    field: Entity,
) -> Option<crate::catalog::TableId> {
    // Build the ancestor chain of field entities, root first.
    let mut chain = Vec::new();
    let mut current = field;
    loop {
        let (_, _, _, parent) = tree.fields_by_entity.get(&current)?;
        chain.push(current);
        match tree.fields_by_entity.get(parent) {
            Some(_) => current = *parent,
            // Parent is the definition entity.
            None => break,
        }
    }
    let root = *chain.last()?;

    // Query roots resolve as tables; fragment bodies against the target.
    let (_, _, _, root_parent) = tree.fields_by_entity.get(&root)?;
    let fragment_target = tree
        .fragments
        .iter()
        .find(|(def_entity, _, _, _)| def_entity == root_parent)
        .map(|(_, _, target, _)| target.name.clone());

    let (_, root_field, _, _) = tree.fields_by_entity.get(&root)?;
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
    for step in chain
        .iter()
        .rev()
        .skip(if fragment_target.is_some() { 0 } else { 1 })
    {
        if *step == field {
            return Some(table);
        }
        let (_, step_field, _, _) = tree.fields_by_entity.get(step)?;
        let reference = FieldRef {
            target: TableRef::parse(&step_field.name),
            selector: step_field.relation_path.as_deref(),
        };
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(table, reference) else {
            return None;
        };
        table = relation.table.id;
    }
    Some(table)
}

/// Renders a resolved selection for hover.
fn describe_target(
    catalog: &crate::catalog::Catalog,
    resolved: &ResolvedSelection,
) -> Option<String> {
    use crate::resolution::SelectionTarget;
    match &resolved.target {
        SelectionTarget::Table(table) => {
            let table = catalog.table_by_id(*table)?;
            Some(format!("table `{}`.`{}`", table.schema, table.name))
        }
        SelectionTarget::Column(column) => {
            let column = catalog.column_by_id(*column)?;
            Some(format!(
                "column `{}`: {}{}",
                column.name,
                column.data_type.as_str(),
                if column.not_null { " (not null)" } else { "" },
            ))
        }
        SelectionTarget::Relation {
            table, selector, ..
        } => {
            let table = catalog.table_by_id(*table)?;
            Some(format!(
                "relation `{}` → `{}`.`{}` via `{selector}`",
                resolved.name, table.schema, table.name,
            ))
        }
        SelectionTarget::Unresolved => None,
    }
}

/// The table a field selection *targets*: the table itself for query
/// roots, or the relation's table for nested selections. This is the
/// context for everything inside the field's braces and clauses.
pub(crate) fn resolve_field_target(
    tree: &SelectionTree<'_>,
    catalog: &crate::catalog::Catalog,
    field_entity: Entity,
) -> Option<crate::catalog::TableId> {
    let (_, field, _, parent) = tree.fields_by_entity.get(&field_entity)?;
    let is_query_root = !tree.fields_by_entity.contains_key(parent)
        && !tree
            .fragments
            .iter()
            .any(|(def_entity, _, _, _)| def_entity == parent);
    if is_query_root {
        return catalog
            .table_ref_for(TableRef::parse(&field.name))
            .map(|table| table.id);
    }

    // Nested and fragment-root fields alike: resolve the containing context
    // (resolve_context_table handles fragment targets), then step through
    // this field's own relation reference.
    let context = resolve_context_table(tree, catalog, field_entity)?;
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
    fn register_completions(reg: &mut Registrar<'_>) {
        reg.system(complete_selections.run_during(bowl::Phase::Complete));
    }
}

/// Contributes tables at query roots and columns/relations inside
/// selection bodies, disambiguating multi-path relations with their
/// `->selector`.
async fn complete_selections(
    requests: Query<(Entity, &CompletionContext), With<CompletionRequest>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    use crate::service::completion::{
        CompletionCandidate, CompletionItem, CompletionKind, CompletionSite,
    };

    let (request, context) = requests.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let mut items = Vec::new();
    let mut push = |item: CompletionItem| items.push(item);

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

    if !items.is_empty() {
        commands.insert((
            DerivedFrom::new(request),
            RequestKey(request),
            CompletionCandidate { items },
        ));
    }
}
