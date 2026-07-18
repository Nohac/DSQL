//! Field-selection entity: one selected field or nested relation inside a
//! selection set, with its alias and relation-path selector — plus the
//! catalog check walk that validates every selection tree top-down.

use std::collections::HashMap;

use bowl::{
    Commands, Component, DerivedFrom, Entity, In, Query, Registrar, SystemExt, SystemParam, View,
    Where, With,
};

use crate::catalog::{CatalogSnapshot, FieldCheckResult, FieldRef, TableRef, TableResolution};
use crate::entities::aggregate::aggregate_output_keys;
use crate::entities::clause::{ClauseFact, clause_expr};
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::expansion::{ExpandedSpread, SpreadExpansion};
use crate::entities::expression::{Expr, LiteralValue};
use crate::entities::fragment_spread::{SpreadDecl, check_spread_site};
use crate::entities::{direct_name, direct_rule, direct_token, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::resolution::{
    FieldResolutions, ResolvedClause, ResolvedSelection, ResolvedSelectionLimit,
    SelectionCardinality, SelectionCardinalityProof, SelectionTarget,
};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{CompletionContext, CompletionRequest};
use crate::service::hover::{
    Cursor, HoverEnriched, describe_column, describe_relation, emit_hover_candidate, priority,
};
use crate::source::{ResolutionScope, ScopeImports};

/// PostgreSQL truncates result aliases beyond this many bytes
/// (`NAMEDATALEN - 1`), which silently corrupts output keys.
pub(crate) const POSTGRES_RESULT_ALIAS_MAX_BYTES: usize = 63;

/// One field selection, lowered from `field_selection`. Together with
/// [`ChildOf`] these facts are the flat encoding of the selection tree;
/// sibling order is byte order of [`FieldSel::span`].
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct FieldSel {
    /// Whether `...` merges this selection's object fields into its parent.
    pub flattened: bool,
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
    /// The result-producing body attached to this source selection.
    pub body: FieldBodyKind,
    /// Whether the selection has a clause list, even an empty one —
    /// scalar fields must not have clauses at all.
    pub has_clause_list: bool,
    /// Normalized output keys contributed by a flattened aggregate body.
    pub aggregate_output_keys: Vec<(String, Span)>,
    /// Shape-affecting clauses summarized from this selection's own CST.
    /// Name resolution turns this syntax into the authoritative semantic
    /// result shape on [`ResolvedSelection`].
    pub shape_syntax: SelectionShapeSyntax,
}

/// The subset of a selection's clauses that can change its row cardinality
/// or nullability. The ordinary [`ClauseFact`] entities remain authoritative
/// for clause checks, variables, planning, and editor services.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct SelectionShapeSyntax {
    /// The effective `where` expression, when written.
    pub predicate: Option<Expr>,
    /// Whether an `offset` may suppress an otherwise singular row.
    pub has_offset: bool,
    /// The effective `limit`, when its syntax can affect shape diagnostics.
    pub limit: SelectionLimitSyntax,
}

/// Shape-relevant syntax of the effective `limit` clause.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum SelectionLimitSyntax {
    /// No valid shape-relevant limit was written.
    None,
    /// A non-negative integer literal.
    Literal { value: u64, span: Span },
    /// A required runtime variable.
    Runtime { span: Span },
}

/// The body attached to a field selection. A pipe transform is distinct from
/// a nested row selection because it changes collection cardinality.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum FieldBodyKind {
    None,
    SelectionSet,
    Transform,
}

impl FieldSel {
    /// The response-object key: an explicit alias, or the selected object's
    /// unqualified name.
    pub(crate) fn output_key(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| TableRef::parse(&self.name).name.to_string())
    }

    pub(crate) fn has_selection_set(&self) -> bool {
        self.body == FieldBodyKind::SelectionSet
    }

    pub(crate) fn has_transform(&self) -> bool {
        self.body == FieldBodyKind::Transform
    }
}

/// Owns `field_selection` (and consumes `field_selection_tail` and
/// `field_suffix` from it).
pub struct FieldSelection;

impl LanguageEntity for FieldSelection {
    const NAME: &'static str = "field_selection";

    fn register(reg: &mut Registrar<'_>) {
        // Views lowered facts ambiently: behind the Complete barrier.
        reg.system(check_selections.run_during(bowl::Phase::Complete));
        reg.system(check_selection_shape);
        reg.system(hover_fields);
        reg.system(complete_selections.run_during(bowl::Phase::Complete));
    }
}

impl LowerStage for FieldSelection {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let flattened = direct_token(ctx.cst, node, Token::Ellipsis).is_some();
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
        let relation_path =
            direct_name(ctx.cst, target).map(|span| text(ctx.source, span).to_string());

        let suffix = tail.and_then(|tail| direct_rule(ctx.cst, tail, Rule::FieldSuffix));
        let body = suffix
            .map(|suffix| {
                if direct_rule(ctx.cst, suffix, Rule::SelectionSet).is_some() {
                    FieldBodyKind::SelectionSet
                } else if direct_rule(ctx.cst, suffix, Rule::PipeTransform).is_some() {
                    FieldBodyKind::Transform
                } else {
                    FieldBodyKind::None
                }
            })
            .unwrap_or(FieldBodyKind::None);
        let has_clause_list = suffix
            .map(|suffix| direct_rule(ctx.cst, suffix, Rule::ClauseList).is_some())
            .unwrap_or(false);
        let aggregate_output_keys = if flattened {
            suffix
                .and_then(|suffix| direct_rule(ctx.cst, suffix, Rule::PipeTransform))
                .map(|transform| aggregate_output_keys(ctx, transform))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let shape_syntax = selection_shape_syntax(ctx, suffix);

        let selection = FieldSel {
            flattened,
            alias,
            alias_span,
            name: text(ctx.source, name_span).to_string(),
            relation_path,
            name_span,
            span: node_span(ctx.cst, node),
            body,
            has_clause_list,
            aggregate_output_keys,
            shape_syntax,
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

fn selection_shape_syntax(ctx: &LowerCtx<'_>, suffix: Option<NodeRef>) -> SelectionShapeSyntax {
    let Some(clause_list) =
        suffix.and_then(|suffix| direct_rule(ctx.cst, suffix, Rule::ClauseList))
    else {
        return SelectionShapeSyntax {
            predicate: None,
            has_offset: false,
            limit: SelectionLimitSyntax::None,
        };
    };

    let mut clauses = ctx
        .cst
        .children(clause_list)
        .filter(|child| ctx.cst.match_rule(*child, Rule::Clause))
        .filter_map(|wrapper| {
            ctx.cst
                .children(wrapper)
                .find(|child| {
                    matches!(
                        ctx.cst.get(*child),
                        crate::grammar::parser::Node::Rule(
                            Rule::WhereClause | Rule::LimitClause | Rule::OffsetClause,
                            _,
                        )
                    )
                })
                .map(|clause| (node_span(ctx.cst, clause).start, clause))
        })
        .collect::<Vec<_>>();
    clauses.sort_by_key(|(start, _)| *start);

    let mut predicate = None;
    let mut has_offset = false;
    let mut limit = SelectionLimitSyntax::None;
    for (_, clause) in clauses {
        if ctx.cst.match_rule(clause, Rule::WhereClause) {
            predicate = Some(clause_expr(ctx, clause));
        } else if ctx.cst.match_rule(clause, Rule::OffsetClause) {
            has_offset = true;
        } else if ctx.cst.match_rule(clause, Rule::LimitClause) {
            let expr = clause_expr(ctx, clause);
            limit = match &expr {
                Expr::Literal {
                    value: LiteralValue::Number(value),
                    span,
                } => value
                    .parse()
                    .ok()
                    .map_or(SelectionLimitSyntax::None, |value| {
                        SelectionLimitSyntax::Literal { value, span: *span }
                    }),
                Expr::Variable { span, .. } => SelectionLimitSyntax::Runtime { span: *span },
                Expr::Literal { .. }
                | Expr::Path { .. }
                | Expr::Aggregate { .. }
                | Expr::Binary { .. }
                | Expr::Unary { .. }
                | Expr::NullTest { .. }
                | Expr::List { .. }
                | Expr::Exists { .. }
                | Expr::PredicateRef { .. }
                | Expr::Error { .. } => SelectionLimitSyntax::None,
            };
        }
    }

    SelectionShapeSyntax {
        predicate,
        has_offset,
        limit,
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
    policies: Query<(
        Entity,
        &crate::entities::policy::PolicyIndex,
        &crate::entities::policy::CompiledPolicyIndex,
    )>,
    imports: Query<(Entity, &ScopeImports)>,
    views: TreeViews<'_>,
    resolutions: View<'_, (Entity, &ResolvedClause)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (def_entity, decl, file, scope) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (_, imports) = imports.item();
    let (policy_index_entity, policy_index, compiled_policies) = policies.item();
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
        policy_index,
        compiled_policies,
        policy_index_entity,
        file: file.0,
        scope: &scope.0,
        enclosing_fragment: None,
        observed_tables: std::collections::HashSet::new(),
        affected_filters: std::collections::HashSet::new(),
        commands: &mut commands,
        imports,
    };

    match decl.kind {
        DefKind::Query => {
            ctx.check_query_roots(def_entity);
            ctx.check_operation_filter_assignments(def_entity);
            ctx.block_unimplemented_filter_execution(def_entity, decl.name_span);
        }
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
    pub(crate) policy_index: &'a crate::entities::policy::PolicyIndex,
    pub(crate) compiled_policies: &'a crate::entities::policy::CompiledPolicyIndex,
    pub(crate) policy_index_entity: Entity,
    pub(crate) file: Entity,
    /// Resolution scope of the definition being checked.
    pub(crate) scope: &'a str,
    /// Name of the fragment whose body is being checked, when any: seeds
    /// spread expansion so self-spreads read as cycles, not duplicates.
    pub(crate) enclosing_fragment: Option<String>,
    pub(crate) observed_tables: std::collections::HashSet<crate::catalog::TableId>,
    pub(crate) affected_filters: std::collections::HashSet<Entity>,
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
        self.diagnostic(
            anchor,
            span,
            Severity::Error,
            DiagnosticSource::Check,
            code,
            message,
        );
    }

    pub(crate) fn warning(
        &mut self,
        anchor: Entity,
        span: Span,
        code: DiagnosticCode,
        message: String,
    ) {
        self.diagnostic(
            anchor,
            span,
            Severity::Warning,
            DiagnosticSource::Check,
            code,
            message,
        );
    }

    fn diagnostic(
        &mut self,
        anchor: Entity,
        span: Span,
        severity: Severity,
        source: DiagnosticSource,
        code: DiagnosticCode,
        message: String,
    ) {
        emit_diagnostic(
            self.commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([
                    anchor,
                    self.catalog_entity,
                    self.policy_index_entity,
                ]),
                file: self.file,
                span,
                severity,
                source,
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
                    self.observed_tables.insert(table_id);
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
                        if field.has_transform() {
                            crate::entities::aggregate::check_source_clause(
                                self,
                                clause_entity,
                                clause,
                                clause_span,
                            );
                        }
                    }
                    if field.flattened && field.body == FieldBodyKind::None {
                        self.missing_flattened_body(entity, field);
                        continue;
                    }
                    if !field.has_selection_set() && !field.has_transform() {
                        self.error(
                            entity,
                            field.name_span,
                            DiagnosticCode::RelationSelectionSet,
                            format!("relation field `{}` must have a selection set", field.name),
                        );
                        continue;
                    }
                    if field.has_selection_set() {
                        self.check_set(table_id, entity);
                    }
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
        self.observed_tables.insert(table);
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
                    if field.has_selection_set() {
                        if field.flattened {
                            self.error(
                                entity,
                                field.name_span,
                                DiagnosticCode::FlattenedSelectionCardinality,
                                format!(
                                    "scalar field `{}` ({}) cannot be flattened",
                                    field.name,
                                    data_type.as_str()
                                ),
                            );
                        } else {
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
                    } else if field.flattened && field.body == FieldBodyKind::None {
                        self.missing_flattened_body(entity, field);
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
                    self.observed_tables.insert(relation_table);
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
                        if field.has_transform() {
                            crate::entities::aggregate::check_source_clause(
                                self,
                                clause_entity,
                                clause,
                                clause_span,
                            );
                        }
                    }
                    if field.flattened && field.body == FieldBodyKind::None {
                        self.missing_flattened_body(entity, field);
                        continue;
                    }
                    if !field.has_selection_set() && !field.has_transform() {
                        self.error(
                            entity,
                            field.name_span,
                            DiagnosticCode::RelationSelectionSet,
                            format!("relation field `{}` must have a selection set", field.name),
                        );
                        continue;
                    }
                    if field.has_selection_set() {
                        self.check_set(relation_table, entity);
                    }
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

    fn check_operation_filter_assignments(&mut self, def_entity: Entity) {
        let mut expansion = SpreadExpansion::new(self.tree, self.scope, self.imports);
        collect_operation_tables(
            self.tree,
            self.clause_resolutions,
            self.catalog,
            self.policy_index,
            self.imports,
            def_entity,
            None,
            self.scope,
            &mut expansion,
            &mut self.observed_tables,
            &mut self.affected_filters,
        );
        let assignments = self
            .tree
            .clauses_under(def_entity)
            .map(|(entity, clause, _, _)| (*entity, (*clause).clone()))
            .collect::<Vec<_>>();
        let mut tables = self.observed_tables.iter().copied().collect::<Vec<_>>();
        tables.sort_by_key(|table| table.0);
        for (entity, clause) in assignments {
            crate::entities::policy::check_operation_filter_assignment(
                self, &tables, entity, &clause,
            );
        }
    }

    fn block_unimplemented_filter_execution(&mut self, query: Entity, name_span: Span) {
        for filter in &self.policy_index.entries {
            if filter.kind != crate::entities::policy::PolicyKind::Filter
                || !filter.default_active
                || !self
                    .imports
                    .visible_from(self.scope)
                    .any(|visible| visible == filter.scope)
                || !self
                    .observed_tables
                    .iter()
                    .any(|table| filter.matches.contains(table))
            {
                continue;
            }
            self.affected_filters.insert(filter.entity);
        }

        let mut filters = self
            .affected_filters
            .iter()
            .filter_map(|entity| self.policy_index.entry(*entity))
            .collect::<Vec<_>>();
        filters.sort_by(|left, right| {
            left.scope
                .cmp(&right.scope)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.entity.cmp(&right.entity))
        });
        for filter in filters {
            let Some(_compiled) = self.compiled_policies.entry(filter.entity) else {
                self.emit_filter_compilation_unavailable(query, name_span, filter);
                continue;
            };
            let missing_target = self
                .observed_tables
                .iter()
                .filter(|table| filter.matches.contains(table))
                .any(|table| {
                    self.compiled_policies
                        .target(filter.entity, *table)
                        .is_none()
                });
            if missing_target {
                self.emit_filter_compilation_unavailable(query, name_span, filter);
                continue;
            }
        }
    }

    fn emit_filter_compilation_unavailable(
        &mut self,
        query: Entity,
        name_span: Span,
        filter: &crate::entities::policy::PolicyEntry,
    ) {
        self.diagnostic(
            query,
            name_span,
            Severity::Error,
            DiagnosticSource::Generate,
            DiagnosticCode::FilterExecutionUnavailable,
            format!(
                "filter `{}` affects this operation but could not be compiled; fix its definition diagnostics",
                filter.name
            ),
        );
    }

    fn missing_flattened_body(&mut self, entity: Entity, field: &FieldSel) {
        self.error(
            entity,
            field.name_span,
            DiagnosticCode::MissingFlattenedSelectionBody,
            format!(
                "flattened selection `{}` must have a selection set or object-producing transform",
                field.name
            ),
        );
    }

    /// Output keys must be unique within one selection set and fit
    /// PostgreSQL's result-alias limit.
    fn check_output_keys(&mut self, parent: Entity) {
        let mut seen: Vec<String> = Vec::new();
        let tree = self.tree;
        let mut expansion = SpreadExpansion::new(tree, self.scope, self.imports);
        if let Some(enclosing) = &self.enclosing_fragment {
            expansion.seed(enclosing);
        }
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect();
        for (entity, field) in fields {
            if field.flattened {
                let mut flattened = Vec::new();
                collect_field_output_keys(tree, &mut expansion, entity, field, &mut flattened);
                dedup_output_keys(&mut flattened);
                for output in flattened {
                    if seen.contains(&output.key) {
                        self.error(
                            entity,
                            output.span,
                            DiagnosticCode::DuplicateOutputKey,
                            format!(
                                "flattened selection `{}` introduces duplicate output key `{}`",
                                field.name, output.key
                            ),
                        );
                    } else {
                        seen.push(output.key);
                    }
                }
                continue;
            }
            let key = field.output_key();
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
        for (entity, name, name_span) in spreads {
            let ExpandedSpread::Fragment {
                entity: fragment, ..
            } = expansion.enter(&name)
            else {
                continue;
            };
            let mut keys = Vec::new();
            collect_selection_output_keys(tree, &mut expansion, fragment, &mut keys);
            expansion.leave();
            dedup_output_keys(&mut keys);
            for output in keys {
                if seen.contains(&output.key) {
                    self.error(
                        entity,
                        name_span,
                        DiagnosticCode::DuplicateOutputKey,
                        format!(
                            "spread `{name}` introduces duplicate output key `{}`; use an alias",
                            output.key
                        ),
                    );
                } else {
                    seen.push(output.key);
                }
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the operation observation walk carries semantic indexes and its two outputs"
)]
fn collect_operation_tables(
    tree: &SelectionTree<'_>,
    resolutions: &std::collections::HashMap<Entity, &ResolvedClause>,
    catalog: &crate::catalog::Catalog,
    policy_index: &crate::entities::policy::PolicyIndex,
    imports: &ScopeImports,
    parent: Entity,
    context: Option<crate::catalog::TableId>,
    definition_scope: &str,
    expansion: &mut SpreadExpansion<'_, '_>,
    tables: &mut std::collections::HashSet<crate::catalog::TableId>,
    filters: &mut std::collections::HashSet<Entity>,
) {
    let fields = tree
        .fields_under(parent)
        .map(|(entity, field, _, _)| (*entity, *field))
        .collect::<Vec<_>>();
    for (entity, field) in fields {
        let table = match context {
            Some(context) => {
                let reference = FieldRef {
                    target: TableRef::parse(&field.name),
                    selector: field.relation_path.as_deref(),
                };
                match catalog.check_field_ref(context, reference) {
                    FieldCheckResult::Relation(relation) => relation.table.id,
                    FieldCheckResult::Column(_)
                    | FieldCheckResult::NotFound
                    | FieldCheckResult::AmbiguousRelation { .. } => continue,
                }
            }
            None => match catalog.resolve_table_ref_for(TableRef::parse(&field.name)) {
                TableResolution::Found(table) => table.id,
                TableResolution::NotFound { .. } | TableResolution::Ambiguous { .. } => continue,
            },
        };
        tables.insert(table);
        for (clause_entity, clause, _, _) in tree.clauses_under(entity) {
            if let ClauseFact::FilterAssignment { name, .. } = clause {
                collect_assignment_filter(
                    policy_index,
                    imports,
                    definition_scope,
                    name,
                    table,
                    filters,
                );
            }
            if let Some(resolved) = resolutions.get(clause_entity) {
                collect_resolved_clause_tables(resolved, tables);
                let expr = match clause {
                    ClauseFact::Where { expr }
                    | ClauseFact::Limit { expr }
                    | ClauseFact::Offset { expr } => Some(expr),
                    ClauseFact::FilterAssignment { condition, .. } => condition.as_ref(),
                    ClauseFact::OrderBy { .. } => None,
                };
                if let Some(expr) = expr {
                    collect_expr_assignment_filters(
                        expr,
                        resolved,
                        policy_index,
                        imports,
                        definition_scope,
                        filters,
                    );
                }
            }
        }
        if field.has_selection_set() {
            collect_operation_tables(
                tree,
                resolutions,
                catalog,
                policy_index,
                imports,
                entity,
                Some(table),
                definition_scope,
                expansion,
                tables,
                filters,
            );
        }
    }

    let spreads = tree
        .spreads_under(parent)
        .map(|(_, spread, _)| spread.name.clone())
        .collect::<Vec<_>>();
    for spread in spreads {
        let ExpandedSpread::Fragment { entity } = expansion.enter(&spread) else {
            continue;
        };
        let fragment_scope = tree
            .fragments
            .iter()
            .find_map(|(fragment, _, _, scope)| (*fragment == entity).then_some(scope.0.as_str()))
            .unwrap_or(definition_scope);
        collect_operation_tables(
            tree,
            resolutions,
            catalog,
            policy_index,
            imports,
            entity,
            context,
            fragment_scope,
            expansion,
            tables,
            filters,
        );
        expansion.leave();
    }
}

fn collect_assignment_filter(
    policy_index: &crate::entities::policy::PolicyIndex,
    imports: &ScopeImports,
    scope: &str,
    name: &str,
    table: crate::catalog::TableId,
    filters: &mut std::collections::HashSet<Entity>,
) {
    let candidates = policy_index.visible(
        scope,
        crate::entities::policy::PolicyKind::Filter,
        name,
        imports,
    );
    if let [filter] = candidates.as_slice()
        && filter.matches.contains(&table)
    {
        filters.insert(filter.entity);
    }
}

fn collect_expr_assignment_filters(
    expr: &Expr,
    resolved: &ResolvedClause,
    policy_index: &crate::entities::policy::PolicyIndex,
    imports: &ScopeImports,
    scope: &str,
    filters: &mut std::collections::HashSet<Entity>,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            collect_expr_assignment_filters(lhs, resolved, policy_index, imports, scope, filters);
            collect_expr_assignment_filters(rhs, resolved, policy_index, imports, scope, filters);
        }
        Expr::Unary { operand, .. } | Expr::NullTest { operand, .. } => {
            collect_expr_assignment_filters(
                operand,
                resolved,
                policy_index,
                imports,
                scope,
                filters,
            );
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_expr_assignment_filters(
                    item,
                    resolved,
                    policy_index,
                    imports,
                    scope,
                    filters,
                );
            }
        }
        Expr::Exists {
            filters: assignments,
            predicate,
            span,
            ..
        } => {
            let table = resolved
                .existence_at(*span)
                .and_then(|existence| existence.source.as_ref())
                .map(|source| match source {
                    crate::resolution::ResolvedExistenceSource::Relation(relation) => {
                        relation.table
                    }
                    crate::resolution::ResolvedExistenceSource::Table(table) => *table,
                });
            if let Some(table) = table {
                for assignment in assignments {
                    collect_assignment_filter(
                        policy_index,
                        imports,
                        scope,
                        &assignment.name,
                        table,
                        filters,
                    );
                }
            }
            if let Some(predicate) = predicate {
                collect_expr_assignment_filters(
                    predicate,
                    resolved,
                    policy_index,
                    imports,
                    scope,
                    filters,
                );
            }
        }
        Expr::Aggregate {
            source, operand, ..
        } => {
            collect_expr_assignment_filters(
                source,
                resolved,
                policy_index,
                imports,
                scope,
                filters,
            );
            if let Some(operand) = operand {
                collect_expr_assignment_filters(
                    operand,
                    resolved,
                    policy_index,
                    imports,
                    scope,
                    filters,
                );
            }
        }
        Expr::Literal { .. }
        | Expr::Path { .. }
        | Expr::Variable { .. }
        | Expr::PredicateRef { .. }
        | Expr::Error { .. } => {}
    }
}

pub(crate) fn collect_resolved_clause_tables(
    resolved: &ResolvedClause,
    tables: &mut std::collections::HashSet<crate::catalog::TableId>,
) {
    for path in &resolved.paths {
        tables.extend(path.relations.iter().map(|relation| relation.table));
        if let crate::resolution::PathTerminal::Column { table, .. } = path.terminal {
            tables.insert(table);
        }
    }
    for aggregate in &resolved.aggregates {
        if let Some(relation) = &aggregate.relation {
            tables.insert(relation.table);
        }
    }
    for existence in &resolved.existences {
        match &existence.source {
            Some(crate::resolution::ResolvedExistenceSource::Relation(relation)) => {
                tables.insert(relation.table);
            }
            Some(crate::resolution::ResolvedExistenceSource::Table(table)) => {
                tables.insert(*table);
            }
            None => {}
        }
    }
}

/// Checks shape-dependent flattening and limit warnings from the tracked
/// [`ResolvedSelection`] fact rather than re-inferring cardinality in the
/// definition walk.
async fn check_selection_shape(
    _: Query<Entity, With<DiagnosticsDemand>>,
    fields: Query<(Entity, &FieldSel, &FieldResolutions, &BelongsToFile)>,
    resolutions: Query<(Entity, &ResolvedSelection), Where<In<FieldResolutions>>>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (field_entity, field, _, file) = fields.item();
    let (resolution_entity, resolved) = resolutions.item();
    let Some(shape) = &resolved.shape else {
        return;
    };

    if field.flattened
        && field.has_selection_set()
        && shape.cardinality == SelectionCardinality::Collection
    {
        let (noun, written) = match &resolved.target {
            SelectionTarget::Table(_) => ("table", field.name.as_str()),
            SelectionTarget::Relation { .. } => ("relation", resolved.written.as_str()),
            SelectionTarget::Column(_) | SelectionTarget::Unresolved => return,
        };
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([field_entity, resolution_entity]),
                file: file.0,
                span: field.name_span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::FlattenedSelectionCardinality,
                message: format!(
                    "{noun} `{written}` is collection-valued and can only flatten through an object-producing transform"
                ),
            },
        );
    }

    let (span, code, message) = match shape.limit {
        ResolvedSelectionLimit::Literal { value: 0, span } => (
            span,
            DiagnosticCode::AlwaysEmptySelection,
            "literal `limit 0` makes this selection always empty".to_string(),
        ),
        ResolvedSelectionLimit::Literal { value, span }
            if matches!(
                shape.proof,
                Some(
                    SelectionCardinalityProof::Relation
                        | SelectionCardinalityProof::UniqueKey(_)
                )
            ) =>
        {
            (
                span,
                DiagnosticCode::RedundantLimit,
                format!(
                    "literal `limit {value}` is redundant because this selection is already at-most-one"
                ),
            )
        }
        ResolvedSelectionLimit::Runtime { span }
            if matches!(
                shape.proof,
                Some(
                    SelectionCardinalityProof::Relation
                        | SelectionCardinalityProof::UniqueKey(_)
                )
            ) =>
        {
            (
                span,
                DiagnosticCode::RedundantLimit,
                "runtime limit cannot further bound this at-most-one selection; it can only suppress the row when zero"
                    .to_string(),
            )
        }
        ResolvedSelectionLimit::None
        | ResolvedSelectionLimit::Literal { .. }
        | ResolvedSelectionLimit::Runtime { .. } => return,
    };
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::many([field_entity, resolution_entity]),
            file: file.0,
            span,
            severity: Severity::Warning,
            source: DiagnosticSource::Check,
            code,
            message,
        },
    );
}

struct OutputKey {
    key: String,
    span: Span,
}

fn dedup_output_keys(keys: &mut Vec<OutputKey>) {
    let mut seen = std::collections::HashSet::new();
    keys.retain(|output| seen.insert(output.key.clone()));
}

/// The top-level output keys a set contributes after fragment expansion and
/// object flattening. The caller owns collision reporting for its parent set.
fn collect_selection_output_keys(
    tree: &SelectionTree<'_>,
    expansion: &mut SpreadExpansion<'_, '_>,
    parent: Entity,
    keys: &mut Vec<OutputKey>,
) {
    for (entity, field, _, _) in tree.fields_under(parent) {
        collect_field_output_keys(tree, expansion, *entity, field, keys);
    }
    let spreads: Vec<String> = tree
        .spreads_under(parent)
        .map(|(_, spread, _)| spread.name.clone())
        .collect();
    for name in spreads {
        if let ExpandedSpread::Fragment { entity, .. } = expansion.enter(&name) {
            collect_selection_output_keys(tree, expansion, entity, keys);
            expansion.leave();
        }
    }
}

fn collect_field_output_keys(
    tree: &SelectionTree<'_>,
    expansion: &mut SpreadExpansion<'_, '_>,
    entity: Entity,
    field: &FieldSel,
    keys: &mut Vec<OutputKey>,
) {
    if !field.flattened {
        keys.push(OutputKey {
            key: field.output_key(),
            span: field.alias_span.unwrap_or(field.name_span),
        });
    } else if field.has_selection_set() {
        collect_selection_output_keys(tree, expansion, entity, keys);
    } else if field.has_transform() {
        keys.extend(
            field
                .aggregate_output_keys
                .iter()
                .map(|(key, span)| OutputKey {
                    key: key.clone(),
                    span: *span,
                }),
        );
    }
}

impl FormatStage for FieldSelection {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        if formatter.direct_token_text(node, Token::Ellipsis).is_some() {
            formatter.write_str("...");
        }
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

    if !resolved.name_span.contains(cursor.0) {
        return;
    }

    let text = describe_target(catalog, resolved).unwrap_or_else(|| format!("`{}`", resolved.name));

    emit_hover_candidate(&mut commands, request, priority::FIELD, text);
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
        SelectionTarget::Column(column) => describe_column(catalog, *column),
        SelectionTarget::Relation {
            table, foreign_key, ..
        } => describe_relation(catalog, &resolved.name, *table, *foreign_key),
        SelectionTarget::Unresolved => None,
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
        CompletionItem, CompletionKind, CompletionSite, emit_completion_candidate,
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

    emit_completion_candidate(&mut commands, request, items);
}
