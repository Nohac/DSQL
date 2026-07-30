//! Reusable filter and condition declarations.
//!
//! Declarations lower as one self-contained fact because their rules are a
//! closed definition body, not part of the query selection tree. The
//! [`PolicyIndex`] resolves catalog targets once and is the tracked input used
//! by checks, planning, metadata, and lock generation.

use std::collections::{BTreeMap, BTreeSet};

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Phase, Query, Registrar, Singleton,
    SystemExt, View, Where, With,
};

use crate::catalog::{
    Catalog, CatalogSnapshot, DataType, FieldCheckResult, FieldRef, RelationCardinality, TableId,
    TableRef, TableResolution, TypeKey, WireEncoding,
};
use crate::entities::definition::DefIndex;
use crate::entities::document::ParsedFile;
use crate::entities::expression::{
    BinaryOp, ExistsSource, Expr, LiteralValue, PathAnchor, PathSegment, Sigil, VariableRef,
    build_expr, expr_child,
};
use crate::entities::{direct_name, direct_names, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{NodeRef, Rule};
use crate::plan::{
    ExistsKind, FilterCollection, FilterColumnScope, FilterExpr, FilterLiteral, FilterOp,
    PolicyContextRequirement, SqlParameter,
};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{
    CompletionContext, CompletionItem, CompletionKind, CompletionRequest, CompletionSite,
    PolicyCompletionContext, PolicyCompletionRole, PolicyCompletionTarget,
    emit_completion_candidate,
};
use crate::service::definition::{DefinitionRequest, DefinitionTarget};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::service::semantic_tokens::{SemanticToken, SemanticTokenKind, TokenChunk, TokensDemand};
use crate::source::{BelongsToHost, ResolutionScope, ScopeImports};

/// Whether one policy declaration defines an applicable filter or a reusable
/// predicate condition.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyKind {
    Filter,
    Condition,
}

impl std::fmt::Display for PolicyKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter => formatter.write_str("filter"),
            Self::Condition => formatter.write_str("condition"),
        }
    }
}

/// A concrete catalog target or structural field contract.
#[derive(Debug, Clone, Hash, PartialEq)]
pub enum PolicyTargetSyntax {
    Concrete { name: String, span: Span },
    Shape { fields: Vec<ShapeField>, span: Span },
}

/// One `.field: logical_type` requirement in a structural target.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ShapeField {
    pub name: String,
    pub name_span: Span,
    pub type_name: String,
    pub type_span: Span,
}

/// The declaration-level application rule. `condition = None` represents a
/// bare `apply`; no [`ApplyRule`] means the filter is manual.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct ApplyRule {
    pub span: Span,
    pub condition: Option<Expr>,
}

/// One scalar or relation field guard.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct PolicyFieldRule {
    pub fields: Vec<(String, Span)>,
    pub condition: Expr,
    pub span: Span,
}

/// A complete filter or reusable-condition declaration.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct PolicyDecl {
    pub kind: PolicyKind,
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub source_hash: u64,
    pub target: Option<PolicyTargetSyntax>,
    pub apply: Option<ApplyRule>,
    pub apply_count: usize,
    pub row_rules: Vec<Expr>,
    pub field_rules: Vec<PolicyFieldRule>,
}

/// Stable resolution failure recorded in [`PolicyIndex`].
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum PolicyTargetProblem {
    Missing,
    EmptyShape,
    UnknownType {
        name: String,
        span: Span,
    },
    DuplicateShapeField {
        name: String,
        span: Span,
    },
    TableNotFound {
        reference: String,
        span: Span,
    },
    AmbiguousTable {
        reference: String,
        candidates: Vec<String>,
        span: Span,
    },
}

/// One declaration with its resolved catalog match set.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PolicyEntry {
    pub entity: Entity,
    pub file: Entity,
    pub scope: String,
    pub kind: PolicyKind,
    pub name: String,
    pub name_span: Span,
    pub matches: Vec<TableId>,
    pub target_fields: Vec<(String, DataType)>,
    pub has_target: bool,
    pub target_problem: Option<PolicyTargetProblem>,
    pub default_active: bool,
    pub always_enforced: bool,
}

/// Tracked catalog and scope resolution for every filter and condition.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct PolicyIndex {
    pub definition_hash: u64,
    pub entries: Vec<PolicyEntry>,
}

/// Body-sensitive tracked input kept separate from [`PolicyIndex`], so edits
/// to rule expressions wake policy compilation without invalidating consumers
/// that only care about definition visibility and catalog matches.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct PolicyBodyIndex {
    pub declarations: Vec<(Entity, PolicyDecl)>,
}

/// One filter compiled for one concrete catalog target.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CompiledPolicyTarget {
    pub table: TableId,
    pub enforcement: Option<FilterExpr>,
    pub row_rule: Option<FilterExpr>,
    pub field_rules: Vec<CompiledPolicyFieldRule>,
    pub context: Vec<PolicyContextRequirement>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum CompiledPolicyField {
    Column(crate::catalog::ColumnId),
    Relation(crate::catalog::RelationId),
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CompiledPolicyFieldRule {
    pub fields: Vec<CompiledPolicyField>,
    pub condition: FilterExpr,
}

/// Query-planning semantics for one filter identity.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CompiledPolicyEntry {
    pub entity: Entity,
    pub scope: String,
    pub name: String,
    pub default_active: bool,
    pub has_field_rules: bool,
    /// Resolved reusable-condition identities referenced by this filter.
    pub conditions: Vec<CompiledPolicyReference>,
    pub targets: Vec<CompiledPolicyTarget>,
}

/// Stable policy-definition identity used by generated audit data.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompiledPolicyReference {
    pub scope: String,
    pub name: String,
}

/// Body-sensitive, catalog-resolved policy semantics consumed by planning.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct CompiledPolicyIndex {
    pub entries: Vec<CompiledPolicyEntry>,
    pub problems: Vec<PolicyCompileProblem>,
}

/// One policy compilation failure that must remain visible to diagnostics.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyCompileProblem {
    pub entity: Entity,
    pub span: Span,
    pub message: String,
}

/// Body-sensitive policy input colocated with [`DefIndex`] so planning keeps
/// one tracked definition join instead of adding another row driver.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct PolicyPlanIndex {
    pub resolution: PolicyIndex,
    pub compiled: CompiledPolicyIndex,
}

impl CompiledPolicyIndex {
    pub fn entry(&self, entity: Entity) -> Option<&CompiledPolicyEntry> {
        self.entries.iter().find(|entry| entry.entity == entity)
    }

    pub fn target(&self, entity: Entity, table: TableId) -> Option<&CompiledPolicyTarget> {
        self.entry(entity)?
            .targets
            .iter()
            .find(|target| target.table == table)
    }
}

impl PolicyIndex {
    pub fn entry(&self, entity: Entity) -> Option<&PolicyEntry> {
        self.entries.iter().find(|entry| entry.entity == entity)
    }

    pub fn visible<'a>(
        &'a self,
        scope: &str,
        kind: PolicyKind,
        name: &str,
        imports: &'a ScopeImports,
    ) -> Vec<&'a PolicyEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.kind == kind
                    && entry.name == name
                    && imports
                        .visible_from(scope)
                        .any(|visible| visible == entry.scope)
            })
            .collect()
    }
}

/// Validates one source-local or operation-level assignment once its owning
/// selection walk has supplied a concrete catalog table.
pub(crate) fn check_filter_assignment(
    context: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    table: TableId,
    entity: Entity,
    clause: &crate::entities::clause::ClauseFact,
) {
    let crate::entities::clause::ClauseFact::FilterAssignment {
        name,
        name_span,
        condition,
    } = clause
    else {
        return;
    };
    let duplicate = context.tree.clauses.values().any(|clauses| {
        clauses.iter().any(|(candidate, candidate_clause, _, _)| {
            *candidate < entity
                && matches!(
                    candidate_clause,
                    crate::entities::clause::ClauseFact::FilterAssignment {
                        name: candidate_name,
                        ..
                    } if candidate_name == name
                )
                && clauses.iter().any(|(current, _, _, _)| *current == entity)
        })
    });
    check_filter_assignment_for_tables(
        context,
        &[table],
        false,
        entity,
        AssignmentCheck {
            name,
            name_span: *name_span,
            condition: condition.as_ref(),
            duplicate,
        },
    );
}

pub(crate) fn check_operation_filter_assignment(
    context: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    tables: &[TableId],
    entity: Entity,
    clause: &crate::entities::clause::ClauseFact,
) {
    let crate::entities::clause::ClauseFact::FilterAssignment {
        name,
        name_span,
        condition,
    } = clause
    else {
        return;
    };
    check_filter_assignment_for_tables(
        context,
        tables,
        true,
        entity,
        AssignmentCheck {
            name,
            name_span: *name_span,
            condition: condition.as_ref(),
            duplicate: false,
        },
    );
}

pub(crate) fn check_exists_filter_assignments(
    context: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    table: TableId,
    entity: Entity,
    filters: &[crate::entities::expression::FilterAssignmentExpr],
) {
    let mut seen = BTreeSet::new();
    for filter in filters {
        let duplicate = !seen.insert(filter.name.as_str());
        check_filter_assignment_for_tables(
            context,
            &[table],
            false,
            entity,
            AssignmentCheck {
                name: &filter.name,
                name_span: filter.name_span,
                condition: filter.condition.as_deref(),
                duplicate,
            },
        );
    }
}

struct AssignmentCheck<'a> {
    name: &'a str,
    name_span: Span,
    condition: Option<&'a Expr>,
    duplicate: bool,
}

fn check_filter_assignment_for_tables(
    context: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    tables: &[TableId],
    operation_wide: bool,
    entity: Entity,
    assignment: AssignmentCheck<'_>,
) {
    let AssignmentCheck {
        name,
        name_span,
        condition,
        duplicate,
    } = assignment;
    let candidates =
        context
            .policy_index
            .visible(context.scope, PolicyKind::Filter, name, context.imports);
    let filter = match candidates.as_slice() {
        [] => {
            context.error(
                entity,
                name_span,
                DiagnosticCode::UnknownFilter,
                format!("filter `{name}` not found"),
            );
            return;
        }
        [filter] => *filter,
        _ => {
            context.error(
                entity,
                name_span,
                DiagnosticCode::AmbiguousFilter,
                format!("filter `{name}` is ambiguous"),
            );
            return;
        }
    };
    let matches = tables.iter().any(|table| filter.matches.contains(table));
    if !matches {
        let target = if operation_wide {
            "any source in this operation".to_string()
        } else {
            let table_name = tables
                .first()
                .and_then(|table| context.catalog.table_by_id(*table))
                .map_or("<unknown>", |table| table.name.as_str());
            format!("table `{table_name}`")
        };
        context.error(
            entity,
            name_span,
            DiagnosticCode::FilterTargetMismatch,
            format!("filter `{name}` does not match {target}"),
        );
    }

    if duplicate {
        context.error(
            entity,
            name_span,
            DiagnosticCode::DuplicateFilterAssignment,
            format!("duplicate assignment for filter `{name}`"),
        );
    }
    if matches {
        context.affected_filters.insert(filter.entity);
    }

    if let Some(condition) = condition {
        if !assignment_expr_is_row_independent(condition) {
            context.error(
                entity,
                condition.span(),
                DiagnosticCode::InvalidFilterAssignment,
                "filter assignment condition must be a row-independent boolean value".to_string(),
            );
        }
        if filter.always_enforced
            && !matches!(
                condition,
                Expr::Literal {
                    value: LiteralValue::Bool(true),
                    ..
                }
            )
        {
            context.error(
                entity,
                condition.span(),
                DiagnosticCode::InvalidFilterAssignment,
                format!("filter `{name}` is always enforced and cannot be disabled"),
            );
        }
        if filter.always_enforced
            && matches!(
                condition,
                Expr::Literal {
                    value: LiteralValue::Bool(true),
                    ..
                }
            )
        {
            context.warning(
                entity,
                condition.span(),
                DiagnosticCode::InvalidFilterAssignment,
                format!("filter `{name}` is always enforced; this assignment is redundant"),
            );
        }
    } else if filter.always_enforced {
        context.warning(
            entity,
            name_span,
            DiagnosticCode::InvalidFilterAssignment,
            format!("filter `{name}` is always enforced; this assignment is redundant"),
        );
    }
}

fn assignment_expr_is_row_independent(expr: &Expr) -> bool {
    match expr {
        Expr::Literal {
            value: LiteralValue::Bool(_),
            ..
        }
        | Expr::Variable { .. } => true,
        Expr::Unary { operand, .. } => assignment_expr_is_row_independent(operand),
        Expr::Binary { op, lhs, rhs, .. } => {
            matches!(
                op,
                crate::entities::expression::BinaryOp::And
                    | crate::entities::expression::BinaryOp::Or
                    | crate::entities::expression::BinaryOp::Comparison(_)
            ) && assignment_expr_is_row_independent(lhs)
                && assignment_expr_is_row_independent(rhs)
        }
        Expr::NullTest { operand, .. } => assignment_expr_is_row_independent(operand),
        Expr::List { .. }
        | Expr::Exists { .. }
        | Expr::Literal { .. }
        | Expr::Path { .. }
        | Expr::DynamicPredicate { .. }
        | Expr::PredicateRef { .. }
        | Expr::Aggregate { .. }
        | Expr::Error { .. } => false,
    }
}

fn unique_visible_policy<'a>(
    index: &'a PolicyIndex,
    imports: &'a ScopeImports,
    scope: &str,
    kind: PolicyKind,
    name: &str,
) -> Option<&'a PolicyEntry> {
    let visible = index.visible(scope, kind, name, imports);
    let [entry] = visible.as_slice() else {
        return None;
    };
    Some(*entry)
}

fn policy_description(
    entry: &PolicyEntry,
    compiled: &CompiledPolicyIndex,
    catalog: &Catalog,
    consumer_scope: &str,
) -> String {
    let mut lines = vec![format!("{} `{}`", entry.kind, entry.name)];
    lines.push(format!("defined in scope `{}`", entry.scope));
    if entry.kind == PolicyKind::Filter {
        lines.push(format!(
            "default: {}",
            if entry.default_active {
                "active"
            } else {
                "inactive"
            }
        ));
        let enforcement = compiled
            .entry(entry.entity)
            .and_then(|filter| filter.targets.first())
            .and_then(|target| target.enforcement.as_ref())
            .map_or("none", |guard| {
                if matches!(guard, FilterExpr::Literal(FilterLiteral::Bool(true))) {
                    "always"
                } else if matches!(guard, FilterExpr::Literal(FilterLiteral::Bool(false))) {
                    "none"
                } else {
                    "conditional"
                }
            });
        lines.push(format!("enforcement: {enforcement}"));
        lines.push(format!(
            "lock: `{consumer_scope} <- {}::{}`",
            entry.scope, entry.name
        ));
    }
    if !entry.matches.is_empty() {
        let mut matches = entry
            .matches
            .iter()
            .filter_map(|table| catalog.table_by_id(*table))
            .map(|table| format!("{}.{}", table.schema, table.name))
            .collect::<Vec<_>>();
        matches.sort();
        lines.push(format!("matches: {}", matches.join(", ")));
    }
    lines.join("\n")
}

fn collect_expr_policy_references(
    expr: &Expr,
    conditions: &mut Vec<(String, Span)>,
    filters: &mut Vec<(String, Span)>,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            collect_expr_policy_references(lhs, conditions, filters);
            collect_expr_policy_references(rhs, conditions, filters);
        }
        Expr::Unary { operand, .. } | Expr::NullTest { operand, .. } => {
            collect_expr_policy_references(operand, conditions, filters);
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_expr_policy_references(item, conditions, filters);
            }
        }
        Expr::Exists {
            source,
            filters: assignments,
            predicate,
            ..
        } => {
            if let ExistsSource::Relation(source) = source {
                collect_expr_policy_references(source, conditions, filters);
            }
            for assignment in assignments {
                filters.push((assignment.name.clone(), assignment.name_span));
                if let Some(condition) = &assignment.condition {
                    collect_expr_policy_references(condition, conditions, filters);
                }
            }
            if let Some(predicate) = predicate {
                collect_expr_policy_references(predicate, conditions, filters);
            }
        }
        Expr::PredicateRef { name, span } => conditions.push((name.clone(), *span)),
        Expr::Aggregate {
            source, operand, ..
        } => {
            collect_expr_policy_references(source, conditions, filters);
            if let Some(operand) = operand {
                collect_expr_policy_references(operand, conditions, filters);
            }
        }
        Expr::Literal { .. }
        | Expr::Path { .. }
        | Expr::Variable { .. }
        | Expr::DynamicPredicate { .. }
        | Expr::Error { .. } => {}
    }
}

fn declaration_references(declaration: &PolicyDecl) -> Vec<(PolicyKind, String, Span)> {
    let mut conditions = Vec::new();
    let mut filters = Vec::new();
    if let Some(condition) = declaration
        .apply
        .as_ref()
        .and_then(|apply| apply.condition.as_ref())
    {
        collect_expr_policy_references(condition, &mut conditions, &mut filters);
    }
    for expression in &declaration.row_rules {
        collect_expr_policy_references(expression, &mut conditions, &mut filters);
    }
    for rule in &declaration.field_rules {
        collect_expr_policy_references(&rule.condition, &mut conditions, &mut filters);
    }
    conditions
        .into_iter()
        .map(|(name, span)| (PolicyKind::Condition, name, span))
        .chain(
            filters
                .into_iter()
                .map(|(name, span)| (PolicyKind::Filter, name, span)),
        )
        .collect()
}

fn clause_filter_references(clause: &crate::entities::clause::ClauseFact) -> Vec<(String, Span)> {
    let mut conditions = Vec::new();
    let mut filters = Vec::new();
    match clause {
        crate::entities::clause::ClauseFact::FilterAssignment {
            name,
            name_span,
            condition,
        } => {
            filters.push((name.clone(), *name_span));
            if let Some(condition) = condition {
                collect_expr_policy_references(condition, &mut conditions, &mut filters);
            }
        }
        crate::entities::clause::ClauseFact::Where { expr }
        | crate::entities::clause::ClauseFact::Limit { expr }
        | crate::entities::clause::ClauseFact::Offset { expr } => {
            collect_expr_policy_references(expr, &mut conditions, &mut filters);
        }
        crate::entities::clause::ClauseFact::OrderBy { .. } => {}
    }
    filters
}

async fn hover_policy_declarations(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    declaration: Query<(Entity, &PolicyDecl, &ResolutionScope), Where<BowlEq<BelongsToFile>>>,
    policies: Query<(Entity, &PolicyIndex, &CompiledPolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = request.item();
    let (entity, declaration, scope) = declaration.item();
    let (_, index, compiled) = policies.item();
    let (_, imports) = imports.item();
    let (_, snapshot) = catalog.item();
    let entry = if declaration.name_span.contains(cursor.0) {
        index.entry(entity)
    } else {
        declaration_references(declaration)
            .into_iter()
            .find(|(_, _, span)| span.contains(cursor.0))
            .and_then(|(kind, name, _)| {
                unique_visible_policy(index, imports, &scope.0, kind, &name)
            })
    };
    if let Some(entry) = entry {
        emit_hover_candidate(
            &mut commands,
            request,
            priority::POLICY,
            policy_description(entry, compiled, snapshot.catalog(), &scope.0),
        );
    }
}

async fn hover_filter_assignments(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    clause: Query<
        (
            Entity,
            &crate::entities::clause::ClauseFact,
            &ResolutionScope,
        ),
        Where<BowlEq<BelongsToFile>>,
    >,
    policies: Query<(Entity, &PolicyIndex, &CompiledPolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, clause, scope) = clause.item();
    let (_, index, compiled) = policies.item();
    let (_, imports) = imports.item();
    let (_, snapshot) = catalog.item();
    let Some((name, _)) = clause_filter_references(clause)
        .into_iter()
        .find(|(_, span)| span.contains(cursor.0))
    else {
        return;
    };
    if let Some(entry) = unique_visible_policy(index, imports, &scope.0, PolicyKind::Filter, &name)
    {
        emit_hover_candidate(
            &mut commands,
            request,
            priority::POLICY,
            policy_description(entry, compiled, snapshot.catalog(), &scope.0),
        );
    }
}

async fn complete_filter_assignments(
    request: Query<(Entity, &CompletionContext), With<CompletionRequest>>,
    policies: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    let (request, context) = request.item();
    if context.site != CompletionSite::FilterAssignment {
        return;
    }
    let (_, index) = policies.item();
    let (_, imports) = imports.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();
    let mut by_name = BTreeMap::<&str, Vec<&PolicyEntry>>::new();
    for entry in index.entries.iter().filter(|entry| {
        entry.kind == PolicyKind::Filter
            && imports
                .visible_from(&context.scope)
                .any(|scope| scope == entry.scope)
            && context
                .table
                .is_none_or(|table| entry.matches.contains(&table))
    }) {
        by_name.entry(&entry.name).or_default().push(entry);
    }
    let items = by_name
        .into_values()
        .filter_map(|entries| {
            let [entry] = entries.as_slice() else {
                return None;
            };
            let targets = entry
                .matches
                .iter()
                .filter_map(|table| catalog.table_by_id(*table))
                .map(|table| format!("{}.{}", table.schema, table.name))
                .collect::<Vec<_>>()
                .join(", ");
            Some(CompletionItem {
                label: entry.name.clone(),
                kind: CompletionKind::Policy,
                detail: Some(format!(
                    "filter {}::{} on {targets}",
                    entry.scope, entry.name
                )),
                documentation: None,
                insert_text: None,
            })
        })
        .collect();
    emit_completion_candidate(&mut commands, request, items);
}

async fn complete_policy_declarations(
    request: Query<(Entity, &PolicyCompletionContext), With<CompletionRequest>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    let (request, context) = request.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();
    let mut items = Vec::new();

    match &context.role {
        PolicyCompletionRole::TargetField { insert_dot } => {
            let mut fields = BTreeMap::<&str, BTreeSet<&str>>::new();
            for table in catalog.visible_tables() {
                for column in catalog.columns_for_table(table.id) {
                    fields
                        .entry(&column.name)
                        .or_default()
                        .insert(catalog.data_type_for_column(column.id).as_str());
                }
            }
            items.extend(fields.into_iter().map(|(name, types)| CompletionItem {
                label: name.to_string(),
                kind: CompletionKind::Column,
                detail: Some(types.into_iter().collect::<Vec<_>>().join(", ")),
                documentation: None,
                insert_text: insert_dot.then(|| format!(".{name}")),
            }));
        }
        PolicyCompletionRole::TargetType { field } => {
            let types = catalog
                .visible_tables()
                .flat_map(|table| catalog.columns_for_table(table.id))
                .filter(|column| column.name == *field)
                .map(|column| catalog.data_type_for_column(column.id).as_str())
                .collect::<BTreeSet<_>>();
            items.extend(types.into_iter().map(|data_type| CompletionItem {
                label: data_type.to_string(),
                kind: CompletionKind::Type,
                detail: Some("logical type".to_string()),
                documentation: None,
                insert_text: None,
            }));
        }
        PolicyCompletionRole::Expression { target } => match target {
            PolicyCompletionTarget::Concrete(table) => {
                items.extend(
                    catalog
                        .columns_for_table(*table)
                        .map(|column| CompletionItem {
                            label: column.name.clone(),
                            kind: CompletionKind::Column,
                            detail: Some(
                                catalog.data_type_for_column(column.id).as_str().to_string(),
                            ),
                            documentation: column.description.clone(),
                            insert_text: None,
                        }),
                );
                let relations = catalog.relation_fields_for_table(*table);
                items.extend(relations.iter().map(|relation| CompletionItem {
                    label: relation.name.to_string(),
                    kind: CompletionKind::Relation,
                    detail: Some(format!(
                        "relation to {}.{} via {}",
                        relation.table.schema, relation.table.name, relation.selector
                    )),
                    documentation: relation.table.description.clone(),
                    insert_text: None,
                }));
            }
            PolicyCompletionTarget::Shape(fields) => {
                let fields = fields
                    .iter()
                    .map(|(name, data_type)| (name.as_str(), data_type.as_str()))
                    .collect::<BTreeMap<_, _>>();
                items.extend(fields.into_iter().map(|(name, data_type)| CompletionItem {
                    label: name.to_string(),
                    kind: CompletionKind::Column,
                    detail: Some(data_type.to_string()),
                    documentation: None,
                    insert_text: None,
                }));
            }
            PolicyCompletionTarget::None => {}
        },
    }

    emit_completion_candidate(&mut commands, request, items);
}

async fn define_policy_references(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    declaration: Query<(Entity, &PolicyDecl, &ResolutionScope), Where<BowlEq<BelongsToFile>>>,
    index: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, declaration, scope) = declaration.item();
    let (_, index) = index.item();
    let (_, imports) = imports.item();
    let Some((kind, name, _)) = declaration_references(declaration)
        .into_iter()
        .find(|(_, _, span)| span.contains(cursor.0))
    else {
        return;
    };
    if let Some(entry) = unique_visible_policy(index, imports, &scope.0, kind, &name) {
        commands.entity(request).insert(DefinitionTarget::Source {
            file: entry.file,
            span: entry.name_span,
        });
    }
}

async fn define_filter_assignments(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    clause: Query<
        (
            Entity,
            &crate::entities::clause::ClauseFact,
            &ResolutionScope,
        ),
        Where<BowlEq<BelongsToFile>>,
    >,
    index: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, clause, scope) = clause.item();
    let (_, index) = index.item();
    let (_, imports) = imports.item();
    let Some((name, _)) = clause_filter_references(clause)
        .into_iter()
        .find(|(_, span)| span.contains(cursor.0))
    else {
        return;
    };
    if let Some(entry) = unique_visible_policy(index, imports, &scope.0, PolicyKind::Filter, &name)
    {
        commands.entity(request).insert(DefinitionTarget::Source {
            file: entry.file,
            span: entry.name_span,
        });
    }
}

async fn policy_declaration_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    declaration: Query<(Entity, &PolicyDecl, &BelongsToFile, &ResolutionScope)>,
    index: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::TokenChunk,)>,
) {
    let demand = demand.item();
    let (entity, declaration, file, scope) = declaration.item();
    let (index_entity, index) = index.item();
    let (_, imports) = imports.item();
    let mut tokens = vec![SemanticToken {
        span: declaration.name_span,
        kind: SemanticTokenKind::Policy,
    }];
    tokens.extend(
        declaration_references(declaration)
            .into_iter()
            .filter(|(kind, name, _)| {
                unique_visible_policy(index, imports, &scope.0, *kind, name).is_some()
            })
            .map(|(_, _, span)| SemanticToken {
                span,
                kind: SemanticTokenKind::Policy,
            }),
    );
    commands.insert((
        DerivedFrom::many([entity, index_entity, demand]),
        BelongsToFile(file.0),
        TokenChunk(tokens),
    ));
}

async fn policy_assignment_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    clause: Query<(
        Entity,
        &crate::entities::clause::ClauseFact,
        &BelongsToFile,
        &ResolutionScope,
    )>,
    index: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::TokenChunk,)>,
) {
    let demand = demand.item();
    let (entity, clause, file, scope) = clause.item();
    let (index_entity, index) = index.item();
    let (_, imports) = imports.item();
    let tokens = clause_filter_references(clause)
        .into_iter()
        .filter(|(name, _)| {
            unique_visible_policy(index, imports, &scope.0, PolicyKind::Filter, name).is_some()
        })
        .map(|(_, span)| SemanticToken {
            span,
            kind: SemanticTokenKind::Policy,
        })
        .collect::<Vec<_>>();
    if !tokens.is_empty() {
        commands.insert((
            DerivedFrom::many([entity, index_entity, demand]),
            BelongsToFile(file.0),
            TokenChunk(tokens),
        ));
    }
}

/// Owns filter and condition definition rules.
pub struct Policy;

impl LanguageEntity for Policy {
    const NAME: &'static str = "policy";

    fn register(registrar: &mut Registrar<'_>) {
        registrar.system(index_policies.run_during(Phase::Complete));
        registrar.system(index_policy_bodies.run_during(Phase::Complete));
        registrar.system(compile_policies.run_during(Phase::Complete));
        registrar.system(check_policy_definitions.run_during(Phase::Complete));
        registrar.system(check_import_ambiguities.run_during(Phase::Complete));
        registrar.system(hover_policy_declarations.run_during(Phase::Complete));
        registrar.system(hover_filter_assignments.run_during(Phase::Complete));
        registrar.system(complete_filter_assignments.run_during(Phase::Complete));
        registrar.system(complete_policy_declarations.run_during(Phase::Complete));
        registrar.system(define_policy_references.run_during(Phase::Complete));
        registrar.system(define_filter_assignments.run_during(Phase::Complete));
        registrar.system(policy_declaration_tokens.run_during(Phase::Complete));
        registrar.system(policy_assignment_tokens.run_during(Phase::Complete));
    }
}

impl LowerStage for Policy {
    fn lower(
        context: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let kind = if context.cst.match_rule(node, Rule::FilterDef) {
            PolicyKind::Filter
        } else {
            PolicyKind::Condition
        };
        let name_span = direct_name(context.cst, node)?;
        let span = node_span(context.cst, node);
        let target = direct_rule(context.cst, node, Rule::PolicyTarget)
            .and_then(|target| lower_target(context, target));
        let mut apply = None;
        let mut apply_count = 0;
        let mut row_rules = Vec::new();
        let mut field_rules = Vec::new();

        if let Some(body) = direct_rule(context.cst, node, Rule::FilterBody) {
            for wrapper in context
                .cst
                .children(body)
                .filter(|child| context.cst.match_rule(*child, Rule::FilterRule))
            {
                if let Some(rule) = direct_rule(context.cst, wrapper, Rule::ApplyRule) {
                    apply_count += 1;
                    apply.get_or_insert_with(|| ApplyRule {
                        span: node_span(context.cst, rule),
                        condition: expr_child(context.cst, rule)
                            .map(|expr| build_expr(context.cst, context.source, expr)),
                    });
                } else if let Some(rule) = direct_rule(context.cst, wrapper, Rule::WhereClause) {
                    row_rules.push(lower_expr(context, rule));
                } else if let Some(rule) = direct_rule(context.cst, wrapper, Rule::FieldRule) {
                    let fields = direct_names(context.cst, rule)
                        .into_iter()
                        .map(|span| (text(context.source, span).to_string(), span))
                        .collect();
                    field_rules.push(PolicyFieldRule {
                        fields,
                        condition: lower_expr(context, rule),
                        span: node_span(context.cst, rule),
                    });
                }
            }
        } else if let Some(body) = direct_rule(context.cst, node, Rule::ConditionBody)
            && let Some(rule) = direct_rule(context.cst, body, Rule::WhereClause)
        {
            row_rules.push(lower_expr(context, rule));
        }

        let decl = PolicyDecl {
            kind,
            name: text(context.source, name_span).to_string(),
            name_span,
            span,
            source_hash: crate::source::content_hash(text(context.source, span)),
            target,
            apply,
            apply_count,
            row_rules,
            field_rules,
        };
        Some(
            commands
                .insert((
                    DerivedFrom::new(context.file),
                    BelongsToFile(context.file),
                    NodeKey {
                        file: context.file,
                        node: node.0,
                    },
                    ResolutionScope(context.scope.to_string()),
                    decl,
                ))
                .untyped(),
        )
    }
}

fn lower_target(context: &LowerCtx<'_>, node: NodeRef) -> Option<PolicyTargetSyntax> {
    if let Some(name) = direct_rule(context.cst, node, Rule::QualifiedName) {
        let span = node_span(context.cst, name);
        return Some(PolicyTargetSyntax::Concrete {
            name: text(context.source, span).to_string(),
            span,
        });
    }
    let shape = direct_rule(context.cst, node, Rule::ShapeTarget)?;
    let fields = context
        .cst
        .children(shape)
        .filter(|child| context.cst.match_rule(*child, Rule::ShapeField))
        .filter_map(|field| {
            let names = direct_names(context.cst, field);
            let [name_span, type_span] = names.as_slice() else {
                return None;
            };
            Some(ShapeField {
                name: text(context.source, *name_span).to_string(),
                name_span: *name_span,
                type_name: text(context.source, *type_span).to_string(),
                type_span: *type_span,
            })
        })
        .collect();
    Some(PolicyTargetSyntax::Shape {
        fields,
        span: node_span(context.cst, shape),
    })
}

fn lower_expr(context: &LowerCtx<'_>, node: NodeRef) -> Expr {
    expr_child(context.cst, node).map_or(
        Expr::Error {
            span: node_span(context.cst, node),
        },
        |expr| build_expr(context.cst, context.source, expr),
    )
}

async fn index_policies(
    _: Query<(Entity, &ParsedFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    defs: Query<(Entity, &DefIndex)>,
    _imports: Query<(Entity, &ScopeImports)>,
    policies: View<'_, (Entity, &PolicyDecl, &BelongsToFile, &ResolutionScope)>,
    mut commands: Commands<(dsql_schema::PolicyIndex,)>,
) {
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();
    let mut entries = policies
        .iter()
        .map(|(entity, decl, file, scope)| resolve_entry(catalog, entity, decl, file.0, &scope.0))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.scope
            .cmp(&right.scope)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.entity.cmp(&right.entity))
    });
    let (_, defs) = defs.item();
    commands.insert((
        Singleton::<PolicyIndex>::new(),
        PolicyIndex {
            definition_hash: defs.content_hash(),
            entries,
        },
    ));
}

async fn index_policy_bodies(
    _: Query<(Entity, &ParsedFile)>,
    policies: View<'_, (Entity, &PolicyDecl)>,
    mut commands: Commands<(dsql_schema::PolicyBodyIndex,)>,
) {
    let mut declarations = policies
        .iter()
        .map(|(entity, declaration)| (entity, declaration.clone()))
        .collect::<Vec<_>>();
    declarations.sort_by_key(|(entity, _)| *entity);
    commands.insert((
        Singleton::<PolicyBodyIndex>::new(),
        PolicyBodyIndex { declarations },
    ));
}

async fn compile_policies(
    policy_index: Query<(Entity, &PolicyIndex)>,
    body_index: Query<(Entity, &PolicyBodyIndex)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    imports: Query<(Entity, &ScopeImports)>,
    definitions: Query<(Entity, &DefIndex)>,
    mut commands: Commands<(dsql_schema::PolicyIndex, dsql_schema::DefIndex)>,
) {
    let (policy_index_entity, policy_index) = policy_index.item();
    let (_, body_index) = body_index.item();
    let (_, snapshot) = catalog.item();
    let (_, imports) = imports.item();
    let (definitions_entity, _) = definitions.item();
    let declarations = body_index
        .declarations
        .iter()
        .map(|(entity, declaration)| (*entity, declaration))
        .collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();
    let mut problems = Vec::new();

    for entry in policy_index
        .entries
        .iter()
        .filter(|entry| entry.kind == PolicyKind::Filter)
    {
        let Some(declaration) = declarations.get(&entry.entity).copied() else {
            continue;
        };
        let mut targets = Vec::new();
        let mut conditions = BTreeSet::new();
        for table in &entry.matches {
            let mut compiler = PolicyCompiler {
                catalog: snapshot.catalog(),
                index: policy_index,
                declarations: &declarations,
                imports,
                scope: &entry.scope,
                context: Vec::new(),
                conditions: BTreeSet::new(),
                problems: Vec::new(),
                failed: false,
            };
            let row = PolicyRowContext::root(*table);
            let enforcement = declaration
                .apply
                .as_ref()
                .and_then(|apply| apply.condition.as_ref())
                .and_then(|condition| {
                    compiler.expr(
                        condition,
                        Some(row),
                        Some(PolicyExpectedType::builtin(DataType::Boolean)),
                    )
                });
            let row_rule = declaration.row_rules.first().and_then(|rule| {
                compiler.expr(
                    rule,
                    Some(row),
                    Some(PolicyExpectedType::builtin(DataType::Boolean)),
                )
            });
            let mut field_rules = Vec::new();
            for rule in &declaration.field_rules {
                let condition = compiler.expr(
                    &rule.condition,
                    Some(row),
                    Some(PolicyExpectedType::builtin(DataType::Boolean)),
                );
                let fields = rule
                    .fields
                    .iter()
                    .map(|(name, _)| compiler.field(*table, name))
                    .collect::<Option<Vec<_>>>();
                if let (Some(condition), Some(fields)) = (condition, fields) {
                    field_rules.push(CompiledPolicyFieldRule { fields, condition });
                }
            }
            problems.extend(compiler.problems.drain(..).map(|(span, message)| {
                PolicyCompileProblem {
                    entity: entry.entity,
                    span,
                    message,
                }
            }));
            if compiler.failed {
                continue;
            }
            compiler
                .context
                .sort_by(|left, right| left.path.cmp(&right.path));
            conditions.extend(compiler.conditions);
            targets.push(CompiledPolicyTarget {
                table: *table,
                enforcement,
                row_rule,
                field_rules,
                context: compiler.context,
            });
        }
        entries.push(CompiledPolicyEntry {
            entity: entry.entity,
            scope: entry.scope.clone(),
            name: entry.name.clone(),
            default_active: entry.default_active,
            has_field_rules: !declaration.field_rules.is_empty(),
            conditions: conditions.into_iter().collect(),
            targets,
        });
    }
    problems.sort();
    problems.dedup();
    let compiled = CompiledPolicyIndex { entries, problems };
    commands
        .entity(policy_index_entity)
        .insert(compiled.clone());
    commands.entity(definitions_entity).insert(PolicyPlanIndex {
        resolution: policy_index.clone(),
        compiled,
    });
}

struct PolicyCompiler<'a> {
    catalog: &'a Catalog,
    index: &'a PolicyIndex,
    declarations: &'a BTreeMap<Entity, &'a PolicyDecl>,
    imports: &'a ScopeImports,
    scope: &'a str,
    context: Vec<PolicyContextRequirement>,
    conditions: BTreeSet<CompiledPolicyReference>,
    problems: Vec<(Span, String)>,
    failed: bool,
}

#[derive(Debug)]
struct CompiledPath {
    value: FilterExpr,
    data_type: DataType,
    wire: WireEncoding,
    provider_type: TypeKey,
    closed_values: Vec<String>,
    relation_scope: FilterColumnScope,
    relations: Vec<(crate::catalog::RelationId, TableId)>,
}

#[derive(Clone, Debug)]
struct PolicyExpectedType {
    data_type: DataType,
    wire: WireEncoding,
    provider_type: Option<TypeKey>,
    closed_values: Vec<String>,
}

impl PolicyExpectedType {
    fn builtin(data_type: DataType) -> Self {
        Self {
            data_type,
            wire: Catalog::builtin_capabilities(data_type).wire,
            provider_type: None,
            closed_values: Vec::new(),
        }
    }

    fn from_path(path: &CompiledPath) -> Self {
        Self {
            data_type: path.data_type,
            wire: path.wire,
            provider_type: Some(path.provider_type.clone()),
            closed_values: path.closed_values.clone(),
        }
    }

    fn text_cast(&self) -> Option<TypeKey> {
        (self.wire == WireEncoding::TextCast)
            .then(|| self.provider_type.clone())
            .flatten()
    }
}

impl PolicyCompiler<'_> {
    fn field(&mut self, table: TableId, name: &str) -> Option<CompiledPolicyField> {
        let reference = FieldRef {
            target: TableRef::parse(name),
            selector: None,
        };
        match self.catalog.check_field_ref(table, reference) {
            FieldCheckResult::Column(column) => Some(CompiledPolicyField::Column(column.id)),
            FieldCheckResult::Relation(relation) => {
                Some(CompiledPolicyField::Relation(relation.relation.id))
            }
            FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => self.fail(),
        }
    }

    fn expr(
        &mut self,
        expr: &Expr,
        row: Option<PolicyRowContext>,
        expected: Option<PolicyExpectedType>,
    ) -> Option<FilterExpr> {
        match expr {
            Expr::Error { .. } | Expr::Aggregate { .. } | Expr::DynamicPredicate { .. } => {
                self.fail()
            }
            Expr::PredicateRef { name, .. } => {
                let candidates =
                    self.index
                        .visible(self.scope, PolicyKind::Condition, name, self.imports);
                let [condition] = candidates.as_slice() else {
                    return self.fail();
                };
                self.conditions.insert(CompiledPolicyReference {
                    scope: condition.scope.clone(),
                    name: condition.name.clone(),
                });
                let Some(declaration) = self.declarations.get(&condition.entity).copied() else {
                    return self.fail();
                };
                let Some(rule) = declaration.row_rules.first() else {
                    return self.fail();
                };
                self.expr(
                    rule,
                    row,
                    Some(PolicyExpectedType::builtin(DataType::Boolean)),
                )
            }
            Expr::Variable { variable, .. } => self.context_parameter(
                variable,
                expected.unwrap_or_else(|| PolicyExpectedType::builtin(DataType::Boolean)),
                false,
            ),
            Expr::Path { .. } => {
                let path = self.path(expr, row)?;
                Some(self.wrap_relations(path.value.clone(), &path))
            }
            Expr::Unary { operand, .. } => self
                .expr(
                    operand,
                    row,
                    Some(PolicyExpectedType::builtin(DataType::Boolean)),
                )
                .map(|operand| FilterExpr::Not(Box::new(operand))),
            Expr::NullTest {
                operand, negated, ..
            } => {
                if matches!(operand.as_ref(), Expr::Path { .. }) {
                    let path = self.path(operand, row)?;
                    let test = FilterExpr::NullTest {
                        operand: Box::new(path.value.clone()),
                        negated: *negated,
                    };
                    return Some(self.wrap_relations(test, &path));
                }
                self.expr(operand, row, None)
                    .map(|operand| FilterExpr::NullTest {
                        operand: Box::new(operand),
                        negated: *negated,
                    })
            }
            Expr::List { .. } => self.fail(),
            Expr::Exists {
                source, predicate, ..
            } => {
                let row = row?;
                let (relation, table) = self.exists_source(source, row)?;
                let filter = predicate
                    .as_deref()
                    .and_then(|predicate| {
                        self.expr(
                            predicate,
                            Some(row.nested(table)),
                            Some(PolicyExpectedType::builtin(DataType::Boolean)),
                        )
                    })
                    .map(Box::new);
                Some(FilterExpr::Exists {
                    relation,
                    table,
                    kind: ExistsKind::Explicit,
                    source_scope: FilterColumnScope::Current,
                    policy_filter: None,
                    field_filters: Vec::new(),
                    filter,
                })
            }
            Expr::Literal { value, span } => {
                if let Some(expected) = expected.as_ref()
                    && !expected.closed_values.is_empty()
                    && !match value {
                        LiteralValue::Null => true,
                        LiteralValue::String(value) => expected.closed_values.contains(value),
                        LiteralValue::Number(_) | LiteralValue::Bool(_) => false,
                    }
                {
                    let identity = expected.provider_type.as_ref().map_or_else(
                        || "closed value".to_string(),
                        |key| format!("{}.{}", key.schema, key.name),
                    );
                    self.problems.push((
                        *span,
                        format!(
                            "expected a variant of `{identity}`; allowed values are {}",
                            expected
                                .closed_values
                                .iter()
                                .map(|value| format!("{value:?}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                    return self.fail();
                }
                Some(FilterExpr::Literal(match value {
                    LiteralValue::String(value) => FilterLiteral::String(value.clone()),
                    LiteralValue::Number(value) => FilterLiteral::Number(value.clone()),
                    LiteralValue::Bool(value) => FilterLiteral::Bool(*value),
                    LiteralValue::Null => FilterLiteral::Null,
                }))
            }
            Expr::Binary { op, lhs, rhs, .. } => self.binary(op, lhs, rhs, row),
        }
    }

    fn binary(
        &mut self,
        op: &BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        row: Option<PolicyRowContext>,
    ) -> Option<FilterExpr> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return Some(FilterExpr::Binary {
                left: Box::new(self.expr(
                    lhs,
                    row,
                    Some(PolicyExpectedType::builtin(DataType::Boolean)),
                )?),
                op: if matches!(op, BinaryOp::And) {
                    FilterOp::And
                } else {
                    FilterOp::Or
                },
                right: Box::new(self.expr(
                    rhs,
                    row,
                    Some(PolicyExpectedType::builtin(DataType::Boolean)),
                )?),
            });
        }

        let path_side = if matches!(lhs, Expr::Path { .. }) {
            Some((lhs, rhs, false))
        } else if matches!(rhs, Expr::Path { .. }) {
            Some((rhs, lhs, true))
        } else {
            None
        };
        if let Some((path_expr, other, reversed)) = path_side {
            let path = self.path(path_expr, row)?;
            if matches!(op, BinaryOp::In | BinaryOp::NotIn) {
                if reversed {
                    return self.fail();
                }
                let collection = match other {
                    Expr::List { items, .. } => FilterCollection::List(
                        items
                            .iter()
                            .map(|item| {
                                self.expr(item, row, Some(PolicyExpectedType::from_path(&path)))
                            })
                            .collect::<Option<Vec<_>>>()?,
                    ),
                    Expr::Variable { variable, .. } => {
                        FilterCollection::Parameter(self.context_parameter_value(
                            variable,
                            PolicyExpectedType::from_path(&path),
                            true,
                        )?)
                    }
                    _ => return self.fail(),
                };
                let membership = FilterExpr::Membership {
                    operand: Box::new(path.value.clone()),
                    collection,
                    negated: matches!(op, BinaryOp::NotIn),
                };
                return Some(self.wrap_relations(membership, &path));
            }
            let other = self.expr(other, row, Some(PolicyExpectedType::from_path(&path)))?;
            let (left, right) = if reversed {
                (other, path.value.clone())
            } else {
                (path.value.clone(), other)
            };
            let binary = FilterExpr::Binary {
                left: Box::new(left),
                op: self.filter_op(op)?,
                right: Box::new(right),
            };
            return Some(self.wrap_relations(binary, &path));
        }

        Some(FilterExpr::Binary {
            left: Box::new(self.expr(lhs, row, None)?),
            op: self.filter_op(op)?,
            right: Box::new(self.expr(rhs, row, None)?),
        })
    }

    fn filter_op(&mut self, op: &BinaryOp) -> Option<FilterOp> {
        match op {
            BinaryOp::Comparison(op) => Some(FilterOp::from(*op)),
            BinaryOp::And => Some(FilterOp::And),
            BinaryOp::Or => Some(FilterOp::Or),
            BinaryOp::In | BinaryOp::NotIn | BinaryOp::Variable(_) => self.fail(),
        }
    }

    fn path(&mut self, expr: &Expr, row: Option<PolicyRowContext>) -> Option<CompiledPath> {
        let Expr::Path {
            anchor, segments, ..
        } = expr
        else {
            return self.fail();
        };
        let row = row?;
        let mut table = match anchor {
            PathAnchor::Current => row.current,
            PathAnchor::Root => row.root,
            PathAnchor::Parent => row.parent?,
        };
        let mut relations = Vec::new();
        for (index, segment) in segments.iter().enumerate() {
            let reference = FieldRef {
                target: TableRef::parse(&segment.name),
                selector: segment.relation_path.as_deref(),
            };
            match self.catalog.check_field_ref(table, reference) {
                FieldCheckResult::Relation(relation) if index + 1 < segments.len() => {
                    relations.push((relation.relation.id, relation.table.id));
                    table = relation.table.id;
                }
                FieldCheckResult::Column(column) if index + 1 == segments.len() => {
                    let catalog_type = self.catalog.type_for_column(column.id)?;
                    let scope = if relations.is_empty() {
                        match anchor {
                            PathAnchor::Current => FilterColumnScope::Current,
                            PathAnchor::Root => FilterColumnScope::Root,
                            PathAnchor::Parent => FilterColumnScope::Parent,
                        }
                    } else {
                        FilterColumnScope::Current
                    };
                    return Some(CompiledPath {
                        value: FilterExpr::Column {
                            scope,
                            column: column.id,
                        },
                        data_type: catalog_type.logical_data_type(),
                        wire: catalog_type.capabilities.wire,
                        provider_type: catalog_type.key.clone(),
                        closed_values: self
                            .catalog
                            .enum_type_for_type(catalog_type.id)
                            .map_or_else(Vec::new, |(_, enumeration)| {
                                enumeration
                                    .variants
                                    .iter()
                                    .map(|variant| variant.variant.clone())
                                    .collect()
                            }),
                        relation_scope: match anchor {
                            PathAnchor::Current => FilterColumnScope::Current,
                            PathAnchor::Root => FilterColumnScope::Root,
                            PathAnchor::Parent => FilterColumnScope::Parent,
                        },
                        relations,
                    });
                }
                FieldCheckResult::Column(_)
                | FieldCheckResult::Relation(_)
                | FieldCheckResult::NotFound
                | FieldCheckResult::AmbiguousRelation { .. } => return self.fail(),
            }
        }
        self.fail()
    }

    fn exists_source(
        &mut self,
        source: &ExistsSource,
        row: PolicyRowContext,
    ) -> Option<(Option<crate::catalog::RelationId>, TableId)> {
        match source {
            ExistsSource::Table { name, .. } => self
                .catalog
                .table_ref_for(TableRef::parse(name))
                .map(|table| (None, table.id))
                .or_else(|| self.fail()),
            ExistsSource::Relation(path) => {
                let Expr::Path {
                    anchor: PathAnchor::Current,
                    segments,
                    ..
                } = path.as_ref()
                else {
                    return self.fail();
                };
                let [segment] = segments.as_slice() else {
                    return self.fail();
                };
                let reference = FieldRef {
                    target: TableRef::parse(&segment.name),
                    selector: segment.relation_path.as_deref(),
                };
                match self.catalog.check_field_ref(row.current, reference) {
                    FieldCheckResult::Relation(relation) => {
                        Some((Some(relation.relation.id), relation.table.id))
                    }
                    FieldCheckResult::Column(_)
                    | FieldCheckResult::NotFound
                    | FieldCheckResult::AmbiguousRelation { .. } => self.fail(),
                }
            }
        }
    }

    fn wrap_relations(&self, filter: FilterExpr, path: &CompiledPath) -> FilterExpr {
        path.relations.iter().enumerate().rev().fold(
            filter,
            |filter, (index, (relation, table))| FilterExpr::Exists {
                relation: Some(*relation),
                table: *table,
                kind: ExistsKind::RelationshipPredicate,
                source_scope: if index == 0 {
                    path.relation_scope
                } else {
                    FilterColumnScope::Current
                },
                policy_filter: None,
                field_filters: Vec::new(),
                filter: Some(Box::new(filter)),
            },
        )
    }

    fn context_parameter(
        &mut self,
        variable: &VariableRef,
        expected: PolicyExpectedType,
        collection: bool,
    ) -> Option<FilterExpr> {
        self.context_parameter_value(variable, expected, collection)
            .map(FilterExpr::Parameter)
    }

    fn context_parameter_value(
        &mut self,
        variable: &VariableRef,
        expected: PolicyExpectedType,
        collection: bool,
    ) -> Option<SqlParameter> {
        if variable.sigil != Sigil::Context {
            return self.fail();
        }
        let name = variable.name.as_deref()?;
        let path = format!("context.{name}");
        if let Some(existing) = self.context.iter().find(|item| item.path == path) {
            let requirement = PolicyContextRequirement {
                path: path.clone(),
                data_type: expected.data_type,
                wire: expected.wire,
                provider_type: expected.provider_type.clone(),
                collection,
            };
            if existing.conflicts_with(&requirement) {
                self.problems
                    .push((variable.span, existing.conflict_message(&requirement)));
                return self.fail();
            }
        } else {
            self.context.push(PolicyContextRequirement {
                path: path.clone(),
                data_type: expected.data_type,
                wire: expected.wire,
                provider_type: expected.provider_type.clone(),
                collection,
            });
        }
        Some(SqlParameter {
            path,
            text_cast: expected.text_cast(),
            collection,
        })
    }

    fn fail<T>(&mut self) -> Option<T> {
        self.failed = true;
        None
    }
}

fn resolve_entry(
    catalog: &Catalog,
    entity: Entity,
    decl: &PolicyDecl,
    file: Entity,
    scope: &str,
) -> PolicyEntry {
    let (matches, target_fields, target_problem) = match &decl.target {
        None if decl.kind == PolicyKind::Condition => (Vec::new(), Vec::new(), None),
        None => (Vec::new(), Vec::new(), Some(PolicyTargetProblem::Missing)),
        Some(PolicyTargetSyntax::Concrete { name, span }) => {
            match catalog.resolve_table_ref_for(TableRef::parse(name)) {
                TableResolution::Found(table) => (vec![table.id], Vec::new(), None),
                TableResolution::NotFound { reference } => (
                    Vec::new(),
                    Vec::new(),
                    Some(PolicyTargetProblem::TableNotFound {
                        reference,
                        span: *span,
                    }),
                ),
                TableResolution::Ambiguous {
                    reference,
                    candidates,
                } => (
                    Vec::new(),
                    Vec::new(),
                    Some(PolicyTargetProblem::AmbiguousTable {
                        reference,
                        candidates: candidates
                            .into_iter()
                            .map(|candidate| format!("{}::{}", candidate.schema, candidate.table))
                            .collect(),
                        span: *span,
                    }),
                ),
            }
        }
        Some(PolicyTargetSyntax::Shape { fields, span }) => {
            resolve_shape_target(catalog, fields, *span)
        }
    };
    let always_enforced = decl.apply.as_ref().is_some_and(|apply| {
        matches!(
            apply.condition,
            Some(Expr::Literal {
                value: LiteralValue::Bool(true),
                ..
            })
        )
    });
    PolicyEntry {
        entity,
        file,
        scope: scope.to_string(),
        kind: decl.kind,
        name: decl.name.clone(),
        name_span: decl.name_span,
        matches,
        target_fields,
        has_target: decl.target.is_some(),
        target_problem,
        default_active: decl.apply.is_some(),
        always_enforced,
    }
}

fn resolve_shape_target(
    catalog: &Catalog,
    fields: &[ShapeField],
    span: Span,
) -> (
    Vec<TableId>,
    Vec<(String, DataType)>,
    Option<PolicyTargetProblem>,
) {
    if fields.is_empty() {
        return (
            Vec::new(),
            Vec::new(),
            Some(PolicyTargetProblem::EmptyShape),
        );
    }
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();
    for field in fields {
        if !seen.insert(field.name.as_str()) {
            return (
                Vec::new(),
                resolved,
                Some(PolicyTargetProblem::DuplicateShapeField {
                    name: field.name.clone(),
                    span: field.name_span,
                }),
            );
        }
        let Some(data_type) = Catalog::resolve_logical_type_name(&field.type_name) else {
            return (
                Vec::new(),
                resolved,
                Some(PolicyTargetProblem::UnknownType {
                    name: field.type_name.clone(),
                    span: field.type_span,
                }),
            );
        };
        resolved.push((field.name.clone(), data_type));
    }
    let matches = catalog
        .tables
        .iter()
        .filter(|table| {
            resolved.iter().all(|(name, data_type)| {
                catalog.columns_for_table(table.id).any(|column| {
                    column.name == *name && catalog.data_type_for_column(column.id) == *data_type
                })
            })
        })
        .map(|table| table.id)
        .collect();
    let _ = span;
    (matches, resolved, None)
}

async fn check_policy_definitions(
    _: Query<Entity, With<DiagnosticsDemand>>,
    policies: Query<(Entity, &PolicyDecl, &BelongsToFile, &ResolutionScope)>,
    index: Query<(Entity, &PolicyIndex, &CompiledPolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    embedded: View<'_, (Entity, &BelongsToHost)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, decl, file, scope) = policies.item();
    let (index_entity, index, compiled) = index.item();
    let (_, imports) = imports.item();
    let (catalog_entity, snapshot) = catalog.item();
    let Some(entry) = index.entry(entity) else {
        return;
    };
    let mut emit = |span: Span, message: String| {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([entity, index_entity, catalog_entity]),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: DiagnosticCode::InvalidPolicyDefinition,
                message,
            },
        );
    };

    if embedded.iter().any(|(document, _)| document == file.0) {
        emit(
            decl.name_span,
            format!(
                "{} definitions must live in standalone dsql files",
                decl.kind
            ),
        );
    }

    for problem in compiled
        .problems
        .iter()
        .filter(|problem| problem.entity == entity)
    {
        emit(problem.span, problem.message.clone());
    }

    diagnose_target_problem(decl, entry, &mut emit);
    if decl.target.is_some() && entry.target_problem.is_none() && entry.matches.is_empty() {
        emit(
            decl.name_span,
            format!("{} `{}` matches no catalog table", decl.kind, decl.name),
        );
    }
    if decl.kind == PolicyKind::Filter && decl.row_rules.is_empty() && decl.field_rules.is_empty() {
        emit(
            decl.span,
            format!("filter `{}` must contain a row or field rule", decl.name),
        );
    }
    if decl.apply_count > 1 {
        emit(
            decl.apply.as_ref().map_or(decl.span, |apply| apply.span),
            format!("filter `{}` may contain only one `apply` rule", decl.name),
        );
    }
    if decl.row_rules.len() > 1 {
        for rule in decl.row_rules.iter().skip(1) {
            emit(
                rule.span(),
                format!(
                    "{} `{}` may contain only one `where` rule",
                    decl.kind, decl.name
                ),
            );
        }
    }
    if decl.kind == PolicyKind::Condition && decl.row_rules.is_empty() {
        emit(
            decl.span,
            format!("condition `{}` must contain one `where` rule", decl.name),
        );
    }

    let same_name = index.visible(&scope.0, decl.kind, &decl.name, imports);
    if same_name.len() > 1 {
        emit(
            decl.name_span,
            format!(
                "{} `{}` is ambiguous in scope `{}`",
                decl.kind, decl.name, scope.0
            ),
        );
    }

    let catalog = snapshot.catalog();
    let row = entry.matches.first().copied().map(PolicyRowContext::root);
    for expr in &decl.row_rules {
        validate_boolean_rule(expr, &mut emit);
        PolicyValidation {
            decl,
            entry,
            index,
            imports,
            catalog,
            scope: &scope.0,
            emit: &mut emit,
        }
        .expr(expr, false, row);
    }
    for field_rule in &decl.field_rules {
        for (field, span) in &field_rule.fields {
            validate_declared_field(decl, entry, field, *span, catalog, &mut emit);
        }
        validate_boolean_rule(&field_rule.condition, &mut emit);
        PolicyValidation {
            decl,
            entry,
            index,
            imports,
            catalog,
            scope: &scope.0,
            emit: &mut emit,
        }
        .expr(&field_rule.condition, false, row);
    }
    if let Some(condition) = decl
        .apply
        .as_ref()
        .and_then(|apply| apply.condition.as_ref())
    {
        validate_boolean_rule(condition, &mut emit);
        PolicyValidation {
            decl,
            entry,
            index,
            imports,
            catalog,
            scope: &scope.0,
            emit: &mut emit,
        }
        .expr(condition, true, None);
    }
}

fn validate_boolean_rule(expr: &Expr, emit: &mut impl FnMut(Span, String)) {
    if !expr_is_boolean(expr) {
        emit(
            expr.span(),
            "filter and condition rules must be boolean predicates".to_string(),
        );
    }
}

fn expr_is_boolean(expr: &Expr) -> bool {
    match expr {
        Expr::Binary { .. }
        | Expr::Unary { .. }
        | Expr::NullTest { .. }
        | Expr::Exists { .. }
        | Expr::PredicateRef { .. }
        | Expr::Variable { .. }
        | Expr::DynamicPredicate { .. }
        | Expr::Literal {
            value: LiteralValue::Bool(_),
            ..
        } => true,
        Expr::Aggregate { function, .. } => function == "exists",
        Expr::List { .. } | Expr::Literal { .. } | Expr::Path { .. } | Expr::Error { .. } => false,
    }
}

fn diagnose_target_problem(
    decl: &PolicyDecl,
    entry: &PolicyEntry,
    emit: &mut impl FnMut(Span, String),
) {
    let Some(problem) = &entry.target_problem else {
        return;
    };
    let (span, message) = match problem {
        PolicyTargetProblem::Missing => (
            decl.name_span,
            format!("filter `{}` must declare an `on` target", decl.name),
        ),
        PolicyTargetProblem::EmptyShape => (
            decl.target.as_ref().map_or(decl.name_span, target_span),
            "structural targets must declare at least one field".to_string(),
        ),
        PolicyTargetProblem::UnknownType { name, span } => (
            *span,
            format!("unknown logical type `{name}` in policy target"),
        ),
        PolicyTargetProblem::DuplicateShapeField { name, span } => {
            (*span, format!("duplicate field `{name}` in policy target"))
        }
        PolicyTargetProblem::TableNotFound { reference, span } => {
            (*span, format!("table `{reference}` not found"))
        }
        PolicyTargetProblem::AmbiguousTable {
            reference,
            candidates,
            span,
        } => (
            *span,
            format!(
                "table `{reference}` is ambiguous; use one of: {}",
                candidates.join(", ")
            ),
        ),
    };
    emit(span, message);
}

fn target_span(target: &PolicyTargetSyntax) -> Span {
    match target {
        PolicyTargetSyntax::Concrete { span, .. } | PolicyTargetSyntax::Shape { span, .. } => *span,
    }
}

#[derive(Clone, Copy)]
struct PolicyRowContext {
    root: TableId,
    current: TableId,
    parent: Option<TableId>,
}

impl PolicyRowContext {
    fn root(table: TableId) -> Self {
        Self {
            root: table,
            current: table,
            parent: None,
        }
    }

    fn nested(self, table: TableId) -> Self {
        Self {
            root: self.root,
            current: table,
            parent: Some(self.current),
        }
    }
}

struct PolicyValidation<'a, Emit> {
    decl: &'a PolicyDecl,
    entry: &'a PolicyEntry,
    index: &'a PolicyIndex,
    imports: &'a ScopeImports,
    catalog: &'a Catalog,
    scope: &'a str,
    emit: &'a mut Emit,
}

impl<Emit: FnMut(Span, String)> PolicyValidation<'_, Emit> {
    fn expr(&mut self, expr: &Expr, context_only: bool, row: Option<PolicyRowContext>) {
        match expr {
            Expr::Variable { variable, .. } => {
                if variable.sigil != Sigil::Context {
                    (self.emit)(
                        variable.span,
                        format!(
                            "{} rules may use trusted `$:` context but not public or build variables",
                            self.decl.kind
                        ),
                    );
                }
            }
            Expr::DynamicPredicate { .. } => (self.emit)(
                expr.span(),
                "bounded dynamic inputs are only supported in query definitions".to_string(),
            ),
            Expr::Path {
                anchor, segments, ..
            } => {
                if context_only {
                    (self.emit)(
                        expr.span(),
                        "`apply where` must depend only on trusted context".to_string(),
                    );
                } else {
                    self.path(*anchor, segments, row);
                }
            }
            Expr::PredicateRef { name, span } => self.condition(name, *span, context_only),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs, context_only, row);
                self.expr(rhs, context_only, row);
            }
            Expr::Unary { operand, .. } | Expr::NullTest { operand, .. } => {
                self.expr(operand, context_only, row);
            }
            Expr::List { items, .. } => {
                for item in items {
                    self.expr(item, context_only, row);
                }
            }
            Expr::Exists {
                source,
                filters,
                predicate,
                ..
            } => {
                for filter in filters {
                    (self.emit)(
                        filter.span,
                        "filter-authored `exists` sources cannot assign query filters".to_string(),
                    );
                }
                let nested = self.exists_source(source, context_only, row, expr.span());
                if let Some(predicate) = predicate
                    && let Some(nested) = nested
                {
                    self.expr(predicate, context_only, Some(nested));
                }
            }
            Expr::Aggregate { .. } => (self.emit)(
                expr.span(),
                "aggregate expressions are not supported in filter definitions yet".to_string(),
            ),
            Expr::Literal { .. } | Expr::Error { .. } => {}
        }
    }

    fn condition(&mut self, name: &str, span: Span, context_only: bool) {
        if self.decl.kind == PolicyKind::Condition {
            (self.emit)(
                span,
                "conditions cannot reference other conditions".to_string(),
            );
            return;
        }
        let candidates = self
            .index
            .visible(self.scope, PolicyKind::Condition, name, self.imports);
        match candidates.as_slice() {
            [] => (self.emit)(span, format!("condition `{name}` not found")),
            [condition] => {
                if context_only && condition.has_target {
                    (self.emit)(
                        span,
                        format!(
                            "condition `{name}` has a row target and cannot be used by `apply where`"
                        ),
                    );
                }
                if !context_only
                    && !condition.matches.is_empty()
                    && self
                        .entry
                        .matches
                        .iter()
                        .any(|table| !condition.matches.contains(table))
                {
                    (self.emit)(
                        span,
                        format!("filter target does not satisfy condition `{name}` target"),
                    );
                }
            }
            _ => (self.emit)(span, format!("condition `{name}` is ambiguous")),
        }
    }

    fn path(
        &mut self,
        anchor: PathAnchor,
        segments: &[PathSegment],
        row: Option<PolicyRowContext>,
    ) {
        let Some(first) = segments.first() else {
            return;
        };
        if matches!(self.decl.target, Some(PolicyTargetSyntax::Shape { .. })) {
            if anchor != PathAnchor::Current || segments.len() != 1 || first.relation_path.is_some()
            {
                (self.emit)(
                    first.span,
                    "relationship and correlated paths require a concrete policy target"
                        .to_string(),
                );
            } else if !self
                .entry
                .target_fields
                .iter()
                .any(|(name, _)| name == &first.name)
            {
                (self.emit)(
                    first.span,
                    format!(
                        "field `{}` must be declared in the structural target",
                        first.name
                    ),
                );
            }
            return;
        }
        let Some(row) = row else {
            (self.emit)(first.span, "row paths require an `on` target".to_string());
            return;
        };
        let Some(mut table) = self.anchor_table(anchor, row, first.span) else {
            return;
        };
        for (index, segment) in segments.iter().enumerate() {
            let reference = FieldRef {
                target: TableRef::parse(&segment.name),
                selector: segment.relation_path.as_deref(),
            };
            match self.catalog.check_field_ref(table, reference) {
                FieldCheckResult::Column(column) if index + 1 == segments.len() => {
                    if self
                        .catalog
                        .type_for_column(column.id)
                        .is_some_and(|data_type| {
                            data_type.capabilities.wire == WireEncoding::Unsupported
                        })
                    {
                        let data_type = self
                            .catalog
                            .type_for_column(column.id)
                            .map_or("unknown", |data_type| data_type.readable_type.as_str());
                        (self.emit)(
                            segment.span,
                            format!("database type `{data_type}` cannot be used as an input"),
                        );
                    }
                    return;
                }
                FieldCheckResult::Relation(relation) if index + 1 < segments.len() => {
                    table = relation.table.id;
                }
                FieldCheckResult::AmbiguousRelation {
                    reference,
                    candidates,
                } => {
                    (self.emit)(
                        segment.span,
                        format!(
                            "relation `{reference}` is ambiguous; use one of: {}",
                            candidates.join(", ")
                        ),
                    );
                    return;
                }
                FieldCheckResult::Column(_)
                | FieldCheckResult::Relation(_)
                | FieldCheckResult::NotFound => {
                    let table_name = self
                        .catalog
                        .table_by_id(table)
                        .map_or("<unknown>", |table| table.name.as_str());
                    (self.emit)(
                        segment.span,
                        format!(
                            "field path `{}` not found on table `{table_name}`",
                            segment.name
                        ),
                    );
                    return;
                }
            }
        }
    }

    fn anchor_table(
        &mut self,
        anchor: PathAnchor,
        row: PolicyRowContext,
        span: Span,
    ) -> Option<TableId> {
        match anchor {
            PathAnchor::Current => Some(row.current),
            PathAnchor::Root => Some(row.root),
            PathAnchor::Parent => match row.parent {
                Some(parent) => Some(parent),
                None => {
                    (self.emit)(span, "this policy expression has no parent row".to_string());
                    None
                }
            },
        }
    }

    fn exists_source(
        &mut self,
        source: &ExistsSource,
        context_only: bool,
        row: Option<PolicyRowContext>,
        span: Span,
    ) -> Option<PolicyRowContext> {
        if context_only {
            (self.emit)(
                span,
                "`apply where` cannot traverse rows or tables".to_string(),
            );
            return None;
        }
        let Some(row) = row else {
            (self.emit)(span, "table traversal requires an `on` target".to_string());
            return None;
        };
        if matches!(self.decl.target, Some(PolicyTargetSyntax::Shape { .. })) {
            (self.emit)(
                span,
                "relationship and table traversal require a concrete policy target".to_string(),
            );
            return None;
        }
        match source {
            ExistsSource::Relation(path) => self.relation_source(path, row),
            ExistsSource::Table { name, span } => {
                match self.catalog.resolve_table_ref_for(TableRef::parse(name)) {
                    TableResolution::Found(table) => Some(row.nested(table.id)),
                    TableResolution::NotFound { reference } => {
                        (self.emit)(*span, format!("table `{reference}` not found"));
                        None
                    }
                    TableResolution::Ambiguous {
                        reference,
                        candidates,
                    } => {
                        let candidates = candidates
                            .iter()
                            .map(|candidate| format!("{}::{}", candidate.schema, candidate.table))
                            .collect::<Vec<_>>();
                        (self.emit)(
                            *span,
                            format!(
                                "table `{reference}` is ambiguous; use one of: {}",
                                candidates.join(", ")
                            ),
                        );
                        None
                    }
                }
            }
        }
    }

    fn relation_source(&mut self, path: &Expr, row: PolicyRowContext) -> Option<PolicyRowContext> {
        let Expr::Path {
            anchor: PathAnchor::Current,
            segments,
            ..
        } = path
        else {
            (self.emit)(
                path.span(),
                "`exists` relation source must be one direct collection relation".to_string(),
            );
            return None;
        };
        let [segment] = segments.as_slice() else {
            (self.emit)(
                path.span(),
                "`exists` relation source must be one direct collection relation".to_string(),
            );
            return None;
        };
        let reference = FieldRef {
            target: TableRef::parse(&segment.name),
            selector: segment.relation_path.as_deref(),
        };
        match self.catalog.check_field_ref(row.current, reference) {
            FieldCheckResult::Relation(relation) => {
                if relation.relation.cardinality != RelationCardinality::Collection {
                    (self.emit)(
                        segment.span,
                        "`exists` relation source must be a collection".to_string(),
                    );
                }
                Some(row.nested(relation.table.id))
            }
            FieldCheckResult::AmbiguousRelation {
                reference,
                candidates,
            } => {
                (self.emit)(
                    segment.span,
                    format!(
                        "relation `{reference}` is ambiguous; use one of: {}",
                        candidates.join(", ")
                    ),
                );
                None
            }
            FieldCheckResult::Column(_) | FieldCheckResult::NotFound => {
                (self.emit)(
                    segment.span,
                    "`exists` relation source must be a collection".to_string(),
                );
                None
            }
        }
    }
}

fn validate_declared_field(
    decl: &PolicyDecl,
    entry: &PolicyEntry,
    field: &str,
    span: Span,
    catalog: &Catalog,
    emit: &mut impl FnMut(Span, String),
) {
    if matches!(decl.target, Some(PolicyTargetSyntax::Shape { .. })) {
        if !entry.target_fields.iter().any(|(name, _)| name == field) {
            emit(
                span,
                format!("field `{field}` must be declared in the structural target"),
            );
        }
        return;
    }
    for table in &entry.matches {
        if !catalog
            .columns_for_table(*table)
            .any(|column| column.name == field)
            && catalog
                .relation_fields_for_table(*table)
                .iter()
                .all(|relation| relation.name != field)
        {
            emit(span, format!("field `{field}` not found on policy target"));
        }
    }
}

async fn check_import_ambiguities(
    _: Query<Entity, With<DiagnosticsDemand>>,
    index: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (index_entity, index) = index.item();
    let (_, imports) = imports.item();
    for consumer in imports.0.keys() {
        let mut groups: BTreeMap<(PolicyKind, &str), Vec<&PolicyEntry>> = BTreeMap::new();
        for entry in &index.entries {
            if imports
                .imports_of(consumer)
                .any(|provider| provider == entry.scope)
            {
                groups
                    .entry((entry.kind, entry.name.as_str()))
                    .or_default()
                    .push(entry);
            }
        }
        for ((kind, name), mut entries) in groups {
            let providers = entries
                .iter()
                .map(|entry| entry.scope.as_str())
                .collect::<BTreeSet<_>>();
            if providers.len() < 2 {
                continue;
            }
            entries.sort_by_key(|entry| (entry.scope.as_str(), entry.name_span.start));
            let first = entries[0];
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::new(index_entity),
                    file: first.file,
                    span: first.name_span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::InvalidPolicyDefinition,
                    message: format!(
                        "{kind} `{name}` is provided to scope `{consumer}` by scopes `{}`",
                        providers.into_iter().collect::<Vec<_>>().join("`, `")
                    ),
                },
            );
        }
    }
}

impl FormatStage for Policy {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.policy_definition(node);
    }
}
