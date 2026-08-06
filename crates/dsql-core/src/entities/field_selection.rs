//! Field-selection entity: one selected field or nested relation inside a
//! selection set, with its alias and relation-path selector — plus the
//! catalog check walk that validates every selection tree top-down.

use std::collections::HashMap;

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, In, Query, Registrar, Related,
    SystemExt, SystemParam, View, Where, With,
};

use crate::catalog::{CatalogSnapshot, TableRef};
use crate::entities::aggregate::aggregate_output_keys;
use crate::entities::clause::{ClauseFact, clause_expr};
use crate::entities::definition::{DefDecl, DefKind, FragmentTarget};
use crate::entities::expansion::{
    ClauseResolutionRows, ExpansionBodies, ExpansionBody, RawSemanticMembers,
    SelectionResolutionRows, SemanticDefinitionKey, SpreadResolutionRows, clone_clause_resolutions,
    clone_selection_resolutions, clone_semantic_members, clone_spread_resolutions,
};
use crate::entities::expression::{Expr, LiteralValue};
use crate::entities::fragment_spread::SpreadDecl;
use crate::entities::{direct_name, direct_rule, direct_token, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, SemanticRoot, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::resolution::{
    FieldResolutions, ResolvedClause, ResolvedSelection, ResolvedSelectionLimit,
    SelectionCardinality, SelectionCardinalityProof, SelectionResolutionProblem, SelectionTarget,
};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{CompletionContext, CompletionRequest};
use crate::service::hover::{
    Cursor, HoverEnriched, describe_column, describe_relation, describe_table,
    emit_hover_candidate, priority,
};
use crate::source::{ResolutionScope, ScopeImports};

/// PostgreSQL truncates result aliases beyond this many bytes
/// (`NAMEDATALEN - 1`), which silently corrupts output keys.
pub(crate) const POSTGRES_RESULT_ALIAS_MAX_BYTES: usize = 63;

/// One field selection, lowered from `field_selection`. Together with
/// [`ChildOf`] these facts are the flat encoding of the selection tree;
/// sibling order is byte order of [`FieldSel::span`].
#[derive(Component, Debug, Clone, Hash)]
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
        reg.system(residual_definition_checks);
        reg.system(check_resolved_selection);
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
                | Expr::DynamicPredicate { .. }
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
/// Spread-site compatibility remains a per-site check, while intrinsic cycle
/// errors are emitted from the materialized expansion graph.
/// The ambient views variable inference, planning, and completion still read,
/// bundled to keep system signatures within porridge's parameter arity. The
/// residual checker consumes relationship-scoped expansion bodies instead.
#[derive(SystemParam)]
pub(crate) struct TreeViews<'a> {
    fields: View<'a, (Entity, &'a FieldSel, &'a NodeKey, &'a ChildOf)>,
    spreads: View<'a, (Entity, &'a SpreadDecl, &'a ChildOf)>,
    fragments: View<'a, (Entity, &'a DefDecl, &'a FragmentTarget, &'a ResolutionScope)>,
    clauses: View<'a, (Entity, &'a ClauseFact, &'a Span, &'a ChildOf)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InstanceNode {
    pub(crate) instance: Option<Entity>,
    pub(crate) source: Entity,
}

#[derive(Default)]
struct DefinitionClosure {
    fields: HashMap<InstanceNode, Vec<(InstanceNode, FieldSel)>>,
    spreads: HashMap<InstanceNode, Vec<(InstanceNode, SpreadDecl)>>,
    clauses: HashMap<InstanceNode, Vec<(InstanceNode, ClauseFact, Span)>>,
    selections: HashMap<InstanceNode, ResolvedSelection>,
    clause_resolutions: HashMap<InstanceNode, ResolvedClause>,
    spread_resolutions: HashMap<InstanceNode, crate::entities::fragment_spread::ResolvedSpread>,
    child_occurrences: HashMap<InstanceNode, Entity>,
    body_roots: HashMap<Entity, Entity>,
    scopes: HashMap<Option<Entity>, String>,
}

impl DefinitionClosure {
    fn node(instance: Option<Entity>, source: Entity) -> InstanceNode {
        InstanceNode { instance, source }
    }

    fn add_members(
        &mut self,
        instance: Option<Entity>,
        members: impl IntoIterator<Item = crate::entities::expansion::ExpansionMember>,
    ) {
        for member in members {
            let node = Self::node(instance, member.source);
            let Some(parent) = member.parent.map(|parent| Self::node(instance, parent)) else {
                continue;
            };
            if let Some(field) = member.field {
                self.fields.entry(parent).or_default().push((node, field));
            }
            if let Some(spread) = member.spread {
                self.spreads.entry(parent).or_default().push((node, spread));
            }
            if let Some(clause) = member.clause {
                let Some(span) = member.span else {
                    continue;
                };
                self.clauses
                    .entry(parent)
                    .or_default()
                    .push((node, clause, span));
            }
        }
    }

    fn fields_under(
        &self,
        parent: InstanceNode,
    ) -> impl Iterator<Item = &(InstanceNode, FieldSel)> {
        self.fields.get(&parent).into_iter().flatten()
    }

    fn spreads_under(
        &self,
        parent: InstanceNode,
    ) -> impl Iterator<Item = &(InstanceNode, SpreadDecl)> {
        self.spreads.get(&parent).into_iter().flatten()
    }

    fn clauses_under(
        &self,
        parent: InstanceNode,
    ) -> impl Iterator<Item = &(InstanceNode, ClauseFact, Span)> {
        self.clauses.get(&parent).into_iter().flatten()
    }

    fn child_root(&self, spread: InstanceNode) -> Option<InstanceNode> {
        let occurrence = self.child_occurrences.get(&spread)?;
        self.body_roots
            .get(occurrence)
            .map(|source| Self::node(Some(*occurrence), *source))
    }

    fn scope_for(&self, node: InstanceNode) -> Option<&str> {
        self.scopes.get(&node.instance).map(String::as_str)
    }

    fn build(
        root_scope: &str,
        root_members: Vec<crate::entities::expansion::ExpansionMember>,
        root_selections: Vec<ResolvedSelection>,
        root_clauses: Vec<ResolvedClause>,
        root_spreads: Vec<crate::entities::fragment_spread::ResolvedSpread>,
        bodies: impl IntoIterator<Item = ExpansionBody>,
    ) -> Self {
        let mut closure = Self::default();
        closure.scopes.insert(None, root_scope.to_string());
        closure.add_members(None, root_members);
        for resolved in root_selections {
            closure
                .selections
                .insert(Self::node(None, resolved.field), resolved);
        }
        for resolved in root_clauses {
            closure
                .clause_resolutions
                .insert(Self::node(None, resolved.clause), resolved);
        }
        for resolved in root_spreads {
            closure
                .spread_resolutions
                .insert(Self::node(None, resolved.spread), resolved);
        }

        for body in bodies {
            let instance = Some(body.occurrence);
            closure.child_occurrences.insert(
                Self::node(body.parent, body.incoming_spread),
                body.occurrence,
            );
            closure.body_roots.insert(body.occurrence, body.definition);
            closure.scopes.insert(instance, body.scope.0.clone());
            closure.add_members(instance, body.members);
            for resolved in body.selections {
                closure
                    .selections
                    .insert(Self::node(instance, resolved.field), resolved);
            }
            for resolved in body.clauses {
                closure
                    .clause_resolutions
                    .insert(Self::node(instance, resolved.clause), resolved);
            }
            for resolved in body.spreads {
                closure
                    .spread_resolutions
                    .insert(Self::node(instance, resolved.spread), resolved);
            }
        }

        for fields in closure.fields.values_mut() {
            fields.sort_by_key(|(_, field)| field.span.start);
        }
        for spreads in closure.spreads.values_mut() {
            spreads.sort_by_key(|(_, spread)| spread.span.start);
        }
        for clauses in closure.clauses.values_mut() {
            clauses.sort_by_key(|(_, _, span)| span.start);
        }
        closure
    }
}

type ResidualRootInput<'a> = Query<(
    Entity,
    &'a SemanticRoot,
    &'a SemanticDefinitionKey,
    &'a NodeKey,
    RawSemanticMembers<'a>,
    SelectionResolutionRows<'a>,
    ClauseResolutionRows<'a>,
    SpreadResolutionRows<'a>,
)>;
type ResidualDefinition<'a> = Query<
    (
        Entity,
        &'a DefDecl,
        Option<&'a FragmentTarget>,
        &'a BelongsToFile,
        &'a ResolutionScope,
    ),
    Where<BowlEq<NodeKey>>,
>;
type ResidualExpansions<'a> = Query<
    (
        Entity,
        &'a SemanticRoot,
        Related<ExpansionBodies, (&'a ExpansionBody,)>,
    ),
    Where<BowlEq<SemanticDefinitionKey>>,
>;

#[expect(
    clippy::too_many_arguments,
    reason = "system parameters are exact tracked joins, not an API surface"
)]
async fn residual_definition_checks(
    _: Query<Entity, With<DiagnosticsDemand>>,
    root_input: ResidualRootInput<'_>,
    definition: ResidualDefinition<'_>,
    expansions: ResidualExpansions<'_>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    policies: Query<(
        Entity,
        &crate::entities::policy::PolicyIndex,
        &crate::entities::policy::CompiledPolicyIndex,
    )>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (root_group, root, _, _, members, selections, clauses, spread_resolutions) =
        root_input.item();
    let (def_entity, decl, fragment_target, file, scope) = definition.item();
    let (expansion_group, _, bodies) = expansions.item();
    debug_assert_eq!(root.0, def_entity);
    debug_assert_eq!(root_group, expansion_group);
    let (catalog_entity, snapshot) = catalog.item();
    let (_, imports) = imports.item();
    let (policy_index_entity, policy_index, compiled_policies) = policies.item();
    let catalog = snapshot.catalog();

    let tree = DefinitionClosure::build(
        &scope.0,
        clone_semantic_members(&members),
        clone_selection_resolutions(&selections),
        clone_clause_resolutions(&clauses),
        clone_spread_resolutions(&spread_resolutions),
        bodies
            .iter()
            .map(|(_, (body,))| (*body).clone())
            .collect::<Vec<_>>(),
    );

    let mut ctx = CheckCtx {
        tree: &tree,
        catalog,
        catalog_entity,
        policy_index,
        compiled_policies,
        policy_index_entity,
        file: file.0,
        scope: &scope.0,
        observed_tables: std::collections::HashSet::new(),
        affected_filters: std::collections::HashSet::new(),
        commands: &mut commands,
        imports,
    };

    match decl.kind {
        DefKind::Query => {
            let root = DefinitionClosure::node(None, def_entity);
            ctx.check_query_roots(root);
            ctx.check_operation_filter_assignments(root);
            ctx.block_unimplemented_filter_execution(def_entity, decl.name_span);
        }
        DefKind::Fragment => {
            ctx.check_fragment_body(DefinitionClosure::node(None, def_entity), fragment_target)
        }
    }
}

/// Shared state of one definition's check walk.
pub(crate) struct CheckCtx<'a> {
    tree: &'a DefinitionClosure,
    pub(crate) catalog: &'a crate::catalog::Catalog,
    pub(crate) catalog_entity: Entity,
    pub(crate) policy_index: &'a crate::entities::policy::PolicyIndex,
    pub(crate) compiled_policies: &'a crate::entities::policy::CompiledPolicyIndex,
    pub(crate) policy_index_entity: Entity,
    pub(crate) file: Entity,
    /// Resolution scope of the definition being checked.
    pub(crate) scope: &'a str,
    pub(crate) observed_tables: std::collections::HashSet<crate::catalog::TableId>,
    pub(crate) affected_filters: std::collections::HashSet<Entity>,
    pub(crate) imports: &'a ScopeImports,
    pub(crate) commands: &'a mut Commands<(dsql_schema::Diagnostic,)>,
}

impl CheckCtx<'_> {
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
    fn check_query_roots(&mut self, definition: InstanceNode) {
        self.check_output_keys(definition);
        let roots: Vec<_> = self
            .tree
            .fields_under(definition)
            .map(|(entity, field)| (*entity, field.clone()))
            .collect();
        for (entity, field) in roots {
            let Some(SelectionTarget::Table(table)) = self
                .tree
                .selections
                .get(&entity)
                .map(|resolved| &resolved.target)
            else {
                continue;
            };
            let table = *table;
            self.observed_tables.insert(table);
            let root_clauses = self
                .tree
                .clauses_under(entity)
                .map(|(entity, clause, _)| (*entity, clause.clone()))
                .collect::<Vec<_>>();
            for (clause_entity, clause) in root_clauses {
                crate::entities::clause::collect_clause_policy_effects(
                    self,
                    table,
                    clause_entity,
                    &clause,
                );
            }
            if field.has_selection_set() {
                self.check_set(table, entity);
            }
        }
    }

    /// Fragment bodies check against the fragment's declared target. An
    /// unresolvable target is reported by the definition entity's own
    /// check; the body is skipped rather than double-reported.
    fn check_fragment_body(&mut self, definition: InstanceNode, target: Option<&FragmentTarget>) {
        let Some(target) = target else {
            return;
        };
        let Some(table) = self.catalog.table_ref_for(TableRef::parse(&target.name)) else {
            return;
        };
        self.check_set(table.id, definition);
    }

    /// Checks one selection set (the children of `parent`) against its
    /// context table, then recurses into relation selections.
    fn check_set(&mut self, table: crate::catalog::TableId, parent: InstanceNode) {
        self.observed_tables.insert(table);
        self.check_output_keys(parent);

        if self.catalog.table_by_id(table).is_none() {
            return;
        }

        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field)| (*entity, field.clone()))
            .collect();
        for (entity, field) in fields {
            let target = self
                .tree
                .selections
                .get(&entity)
                .map(|resolved| resolved.target.clone());
            match target {
                Some(SelectionTarget::Column(_)) => {}
                Some(SelectionTarget::Relation {
                    table: relation_table,
                    ..
                }) => {
                    self.observed_tables.insert(relation_table);
                    let field_clauses: Vec<_> = self
                        .tree
                        .clauses_under(entity)
                        .map(|(entity, clause, _)| (*entity, clause.clone()))
                        .collect();
                    for (clause_entity, clause) in field_clauses {
                        crate::entities::clause::collect_clause_policy_effects(
                            self,
                            relation_table,
                            clause_entity,
                            &clause,
                        );
                    }
                    if field.has_selection_set() {
                        self.check_set(relation_table, entity);
                    }
                }
                Some(SelectionTarget::Table(_)) | Some(SelectionTarget::Unresolved) | None => {}
            }
        }

        let spreads: Vec<_> = self
            .tree
            .spreads_under(parent)
            .map(|(entity, spread)| (*entity, spread.clone()))
            .collect();
        for (entity, spread) in spreads {
            self.check_spread_site(entity, &spread, table);
        }
    }

    fn check_spread_site(
        &mut self,
        spread_node: InstanceNode,
        spread: &SpreadDecl,
        context_table: crate::catalog::TableId,
    ) {
        let Some(target_name) = self
            .tree
            .spread_resolutions
            .get(&spread_node)
            .and_then(|resolved| resolved.target.as_ref())
            .and_then(|target| target.on.as_deref())
        else {
            return;
        };
        let Some(target_table) = self.catalog.table_ref_for(TableRef::parse(target_name)) else {
            return;
        };
        if target_table.id == context_table {
            return;
        }
        let context_name = self
            .catalog
            .table_by_id(context_table)
            .map(|table| table.name.clone())
            .unwrap_or_default();
        self.error(
            spread_node.source,
            spread.name_span,
            DiagnosticCode::FragmentTypeMismatch,
            format!(
                "fragment `{}` applies to `{}` and cannot be spread in `{context_name}`",
                spread.name, target_table.name
            ),
        );
    }

    pub(crate) fn resolved_clause(&self, clause: InstanceNode) -> Option<&ResolvedClause> {
        self.tree.clause_resolutions.get(&clause)
    }

    pub(crate) fn is_duplicate_filter_assignment(&self, clause: InstanceNode, name: &str) -> bool {
        self.tree.clauses.values().any(|clauses| {
            clauses.iter().any(|(candidate, candidate_clause, _)| {
                candidate.instance == clause.instance
                    && candidate.source < clause.source
                    && matches!(
                        candidate_clause,
                        ClauseFact::FilterAssignment {
                            name: candidate_name,
                            ..
                        } if candidate_name == name
                    )
                    && clauses.iter().any(|(current, _, _)| *current == clause)
            })
        })
    }

    fn check_operation_filter_assignments(&mut self, definition: InstanceNode) {
        collect_operation_tables(
            self.tree,
            self.policy_index,
            self.imports,
            definition,
            &mut self.observed_tables,
            &mut self.affected_filters,
        );
        let assignments = self
            .tree
            .clauses_under(definition)
            .map(|(entity, clause, _)| (*entity, clause.clone()))
            .collect::<Vec<_>>();
        let mut tables = self.observed_tables.iter().copied().collect::<Vec<_>>();
        tables.sort_by_key(|table| table.0);
        for (entity, clause) in assignments {
            crate::entities::policy::check_operation_filter_assignment(
                self,
                &tables,
                entity.source,
                &clause,
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

    /// Output keys must be unique within one selection set and fit
    /// PostgreSQL's result-alias limit.
    fn check_output_keys(&mut self, parent: InstanceNode) {
        let mut seen: Vec<String> = Vec::new();
        let tree = self.tree;
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field)| (*entity, field.clone()))
            .collect();
        for (entity, field) in fields {
            if field.flattened {
                let mut flattened = Vec::new();
                collect_closure_field_output_keys(tree, entity, &field, &mut flattened);
                dedup_output_keys(&mut flattened);
                for output in flattened {
                    if seen.contains(&output.key) {
                        self.error(
                            entity.source,
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
                    entity.source,
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
                    entity.source,
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
            .map(|(entity, spread)| (*entity, spread.name.clone(), spread.name_span))
            .collect();
        for (entity, name, name_span) in spreads {
            let Some(fragment) = tree.child_root(entity) else {
                continue;
            };
            let mut keys = Vec::new();
            collect_closure_selection_output_keys(tree, fragment, &mut keys);
            dedup_output_keys(&mut keys);
            for output in keys {
                if seen.contains(&output.key) {
                    self.error(
                        entity.source,
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

fn collect_operation_tables(
    tree: &DefinitionClosure,
    policy_index: &crate::entities::policy::PolicyIndex,
    imports: &ScopeImports,
    parent: InstanceNode,
    tables: &mut std::collections::HashSet<crate::catalog::TableId>,
    filters: &mut std::collections::HashSet<Entity>,
) {
    let definition_scope = tree.scope_for(parent).unwrap_or_default();
    let fields = tree
        .fields_under(parent)
        .map(|(entity, field)| (*entity, field.clone()))
        .collect::<Vec<_>>();
    for (entity, field) in fields {
        let Some(table) = tree
            .selections
            .get(&entity)
            .and_then(|resolved| resolved.target.child_context())
        else {
            continue;
        };
        tables.insert(table);
        for (clause_entity, clause, _) in tree.clauses_under(entity) {
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
            if let Some(resolved) = tree.clause_resolutions.get(clause_entity) {
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
            collect_operation_tables(tree, policy_index, imports, entity, tables, filters);
        }
    }

    let spreads = tree
        .spreads_under(parent)
        .map(|(spread, _)| *spread)
        .collect::<Vec<_>>();
    for spread in spreads {
        let Some(fragment) = tree.child_root(spread) else {
            continue;
        };
        collect_operation_tables(tree, policy_index, imports, fragment, tables, filters);
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
        | Expr::DynamicPredicate { .. }
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

/// Emits diagnostics owned by one exact field-resolution pair.
///
/// Resolution failures and scalar/relation body rules depend only on the
/// normalized [`ResolvedSelection`] value. They therefore stay phase-free and
/// do not reopen the project-wide selection tree or catalog.
async fn check_resolved_selection(
    _: Query<Entity, With<DiagnosticsDemand>>,
    fields: Query<(Entity, &FieldSel, &FieldResolutions, &BelongsToFile)>,
    resolutions: Query<(Entity, &ResolvedSelection), Where<In<FieldResolutions>>>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (field_entity, field, _, file) = fields.item();
    let (resolution_entity, resolved) = resolutions.item();

    let mut emit = |span, code, message| {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([field_entity, resolution_entity]),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code,
                message,
            },
        );
    };

    if let Some(problem) = &resolved.problem {
        let (code, message) = match problem {
            SelectionResolutionProblem::TableNotFound { reference } => (
                DiagnosticCode::TableNotFound,
                format!("table `{reference}` not found"),
            ),
            SelectionResolutionProblem::AmbiguousTable {
                reference,
                candidates,
            } => (
                DiagnosticCode::AmbiguousTable,
                format!(
                    "table `{reference}` is ambiguous; use an alias with a schema-qualified name ({})",
                    candidates
                        .iter()
                        .map(|key| format!("{}::{}", key.schema, key.table))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            SelectionResolutionProblem::FieldNotFound { reference, table } => (
                DiagnosticCode::FieldNotFound,
                format!("field `{reference}` not found on table `{table}`"),
            ),
            SelectionResolutionProblem::AmbiguousRelation {
                reference,
                candidates,
            } => (
                DiagnosticCode::AmbiguousRelation,
                format!(
                    "relation `{reference}` has multiple foreign-key paths; use one of: {}",
                    candidates.join(", ")
                ),
            ),
        };
        emit(field.name_span, code, message);
        return;
    }

    match &resolved.target {
        SelectionTarget::Table(_) | SelectionTarget::Relation { .. } => {
            if field.flattened && field.body == FieldBodyKind::None {
                emit(
                    field.name_span,
                    DiagnosticCode::MissingFlattenedSelectionBody,
                    format!(
                        "flattened selection `{}` must have a selection set or object-producing transform",
                        field.name
                    ),
                );
            } else if !field.has_selection_set() && !field.has_transform() {
                emit(
                    field.name_span,
                    DiagnosticCode::RelationSelectionSet,
                    format!("relation field `{}` must have a selection set", field.name),
                );
            }
        }
        SelectionTarget::Column(_) => {
            let data_type = resolved
                .value_type
                .as_ref()
                .map_or("unknown", |value_type| value_type.logical.as_str());
            if field.has_selection_set() {
                let (code, message) = if field.flattened {
                    (
                        DiagnosticCode::FlattenedSelectionCardinality,
                        format!(
                            "scalar field `{}` ({data_type}) cannot be flattened",
                            field.name
                        ),
                    )
                } else {
                    (
                        DiagnosticCode::ScalarSelectionSet,
                        format!(
                            "field `{}` is a scalar ({data_type}) and cannot have a selection set",
                            field.name
                        ),
                    )
                };
                emit(field.name_span, code, message);
            } else if field.flattened && field.body == FieldBodyKind::None {
                emit(
                    field.name_span,
                    DiagnosticCode::MissingFlattenedSelectionBody,
                    format!(
                        "flattened selection `{}` must have a selection set or object-producing transform",
                        field.name
                    ),
                );
            }
            if field.has_clause_list {
                emit(
                    field.name_span,
                    DiagnosticCode::ScalarClauses,
                    format!(
                        "field `{}` is a scalar ({data_type}); only relations can have clauses",
                        field.name
                    ),
                );
            }
        }
        SelectionTarget::Unresolved => {}
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
fn collect_closure_selection_output_keys(
    tree: &DefinitionClosure,
    parent: InstanceNode,
    keys: &mut Vec<OutputKey>,
) {
    for (entity, field) in tree.fields_under(parent) {
        collect_closure_field_output_keys(tree, *entity, field, keys);
    }
    let spreads = tree
        .spreads_under(parent)
        .map(|(entity, _)| *entity)
        .collect::<Vec<_>>();
    for spread in spreads {
        if let Some(root) = tree.child_root(spread) {
            collect_closure_selection_output_keys(tree, root, keys);
        }
    }
}

fn collect_closure_field_output_keys(
    tree: &DefinitionClosure,
    entity: InstanceNode,
    field: &FieldSel,
    keys: &mut Vec<OutputKey>,
) {
    if !field.flattened {
        keys.push(OutputKey {
            key: field.output_key(),
            span: field.alias_span.unwrap_or(field.name_span),
        });
    } else if field.has_selection_set() {
        collect_closure_selection_output_keys(tree, entity, keys);
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

    let Some(text) = describe_target(catalog, resolved) else {
        return;
    };

    emit_hover_candidate(&mut commands, request, priority::FIELD, text);
}

/// Renders a resolved selection for hover.
fn describe_target(
    catalog: &crate::catalog::Catalog,
    resolved: &ResolvedSelection,
) -> Option<String> {
    use crate::resolution::SelectionTarget;
    match &resolved.target {
        SelectionTarget::Table(table) => describe_table(catalog, *table),
        SelectionTarget::Column(column) => describe_column(catalog, *column),
        SelectionTarget::Relation {
            table, relation, ..
        } => describe_relation(catalog, &resolved.name, *table, *relation),
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
            for table in catalog.visible_tables() {
                let label = if table.schema == catalog.default_schema() {
                    table.name.clone()
                } else {
                    format!("{}::{}", table.schema, table.name)
                };
                push(CompletionItem {
                    label,
                    kind: CompletionKind::Table,
                    detail: Some(format!("table {}.{}", table.schema, table.name)),
                    documentation: table.description.clone(),
                    insert_text: None,
                });
            }
        }
        (CompletionSite::SelectionBody, Some(table)) => {
            for column in catalog.columns_for_table(table) {
                push(CompletionItem {
                    label: column.name.clone(),
                    kind: CompletionKind::Column,
                    detail: Some(column.formatted_type.clone()),
                    documentation: column.description.clone(),
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
                    documentation: relation.table.description.clone(),
                    insert_text: None,
                });
            }
        }
        _ => {}
    }

    emit_completion_candidate(&mut commands, request, items);
}
