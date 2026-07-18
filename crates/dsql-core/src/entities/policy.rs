//! Reusable filter and condition declarations.
//!
//! Declarations lower as one self-contained fact because their rules are a
//! closed definition body, not part of the query selection tree. The
//! [`PolicyIndex`] resolves catalog targets once and is the tracked input used
//! by checks, planning, metadata, and lock generation.

use std::collections::{BTreeMap, BTreeSet};

use bowl::{
    Commands, Component, DerivedFrom, Entity, Phase, Query, Registrar, Singleton, SystemExt, View,
    With,
};

use crate::catalog::{
    Catalog, CatalogSnapshot, DataType, FieldCheckResult, FieldRef, RelationCardinality, TableId,
    TableRef, TableResolution,
};
use crate::entities::definition::DefIndex;
use crate::entities::document::ParsedFile;
use crate::entities::expression::{
    ExistsSource, Expr, LiteralValue, PathAnchor, PathSegment, Sigil, build_expr, expr_child,
};
use crate::entities::{direct_name, direct_names, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{NodeRef, Rule};
use crate::schema::{AstFacts, dsql_schema};
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
        | Expr::PredicateRef { .. }
        | Expr::Aggregate { .. }
        | Expr::Error { .. } => false,
    }
}

/// Owns filter and condition definition rules.
pub struct Policy;

impl LanguageEntity for Policy {
    const NAME: &'static str = "policy";

    fn register(registrar: &mut Registrar<'_>) {
        registrar.system(index_policies.run_during(Phase::Complete));
        registrar.system(check_policy_definitions.run_during(Phase::Complete));
        registrar.system(check_import_ambiguities.run_during(Phase::Complete));
        registrar.system(diagnose_unbound_trusted_context.run_during(Phase::Complete));
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
        let data_type = logical_type(&field.type_name);
        if data_type == DataType::Unknown && field.type_name != "unknown" {
            return (
                Vec::new(),
                resolved,
                Some(PolicyTargetProblem::UnknownType {
                    name: field.type_name.clone(),
                    span: field.type_span,
                }),
            );
        }
        resolved.push((field.name.clone(), data_type));
    }
    let matches = catalog
        .tables
        .iter()
        .filter(|table| {
            resolved.iter().all(|(name, data_type)| {
                catalog
                    .columns_for_table(table.id)
                    .any(|column| column.name == *name && column.data_type == *data_type)
            })
        })
        .map(|table| table.id)
        .collect();
    let _ = span;
    (matches, resolved, None)
}

fn logical_type(name: &str) -> DataType {
    match name {
        "uuid" => DataType::Uuid,
        "text" => DataType::Text,
        "timestamptz" => DataType::Timestamptz,
        "int" => DataType::Int,
        "numeric" => DataType::Numeric,
        "float" => DataType::Float,
        "boolean" => DataType::Boolean,
        "json" => DataType::Json,
        "unknown" => DataType::Unknown,
        other => DataType::from_database_type(other),
    }
}

async fn check_policy_definitions(
    _: Query<Entity, With<DiagnosticsDemand>>,
    policies: Query<(Entity, &PolicyDecl, &BelongsToFile, &ResolutionScope)>,
    index: Query<(Entity, &PolicyIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    embedded: View<'_, (Entity, &BelongsToHost)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, decl, file, scope) = policies.item();
    let (index_entity, index) = index.item();
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
                FieldCheckResult::Column(_) if index + 1 == segments.len() => return,
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
                let cardinality = self
                    .catalog
                    .foreign_key_by_id(relation.foreign_key.id)
                    .and_then(|foreign_key| {
                        self.catalog.relation_cardinality(
                            row.current,
                            relation.table.id,
                            foreign_key,
                        )
                    });
                if cardinality != Some(RelationCardinality::Collection) {
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

async fn diagnose_unbound_trusted_context(
    _: Query<Entity, With<DiagnosticsDemand>>,
    policies: Query<(Entity, &PolicyDecl, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, decl, file) = policies.item();
    let mut variables = Vec::new();
    for expr in decl
        .row_rules
        .iter()
        .chain(decl.field_rules.iter().map(|rule| &rule.condition))
        .chain(
            decl.apply
                .iter()
                .filter_map(|apply| apply.condition.as_ref()),
        )
    {
        collect_context_variables(expr, &mut variables);
    }
    for (name, span) in variables {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(entity),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Generate,
                code: DiagnosticCode::TrustedContextBindingUnavailable,
                message: format!(
                    "trusted context `{name}` cannot be bound until the server-only execution boundary is available"
                ),
            },
        );
    }
}

fn collect_context_variables(expr: &Expr, variables: &mut Vec<(String, Span)>) {
    match expr {
        Expr::Variable { variable, .. } if variable.sigil == Sigil::Context => {
            variables.push((
                variable
                    .name
                    .clone()
                    .unwrap_or_else(|| "<anonymous>".to_string()),
                variable.span,
            ));
        }
        Expr::Binary { lhs, rhs, .. } => {
            collect_context_variables(lhs, variables);
            collect_context_variables(rhs, variables);
        }
        Expr::Unary { operand, .. } | Expr::NullTest { operand, .. } => {
            collect_context_variables(operand, variables);
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_context_variables(item, variables);
            }
        }
        Expr::Exists {
            source,
            filters,
            predicate,
            ..
        } => {
            for filter in filters {
                if let Some(condition) = &filter.condition {
                    collect_context_variables(condition, variables);
                }
            }
            if let crate::entities::expression::ExistsSource::Relation(source) = source {
                collect_context_variables(source, variables);
            }
            if let Some(predicate) = predicate {
                collect_context_variables(predicate, variables);
            }
        }
        Expr::Aggregate {
            source, operand, ..
        } => {
            collect_context_variables(source, variables);
            if let Some(operand) = operand {
                collect_context_variables(operand, variables);
            }
        }
        Expr::Literal { .. }
        | Expr::Path { .. }
        | Expr::Variable { .. }
        | Expr::PredicateRef { .. }
        | Expr::Error { .. } => {}
    }
}

impl FormatStage for Policy {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.policy_definition(node);
    }
}
