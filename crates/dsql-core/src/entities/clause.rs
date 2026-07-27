//! Clause entity: the `(where ... order by ... limit ... offset ...)`
//! clause list attached to a field selection.
//!
//! One entity covers all four clause kinds — they share shape, checks, and
//! planning surface; [`ClauseFact`] branches where they differ.

use std::borrow::Cow;

use crate::schema::{AstFacts, dsql_schema};
use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, Registrar, SystemExt, Where,
    With,
};

use crate::catalog::CatalogSnapshot;
use crate::entities::expression::{
    DynamicInputSurface, Expr, PathAnchor, VariableRef, build_expr, build_filter_assignment,
    build_variable_ref, dynamic_input_surface, expr_child,
};
use crate::entities::{direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{BelongsToFile, ChildOf, NodeKey, Span};
use crate::format::CstFormatter;
use crate::grammar::parser::{CstData, NodeRef, Rule};
use crate::resolution::{PathTerminal, ResolvedClause};
use crate::service::hover::{
    Cursor, HoverEnriched, describe_column, describe_relation, emit_hover_candidate, priority,
};
use crate::source::ResolutionScope;

/// One clause, lowered from `where_clause` / `order_by_clause` /
/// `limit_clause` / `offset_clause`. [`ChildOf`] links it to the field
/// selection it constrains.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub enum ClauseFact {
    FilterAssignment {
        name: String,
        name_span: Span,
        condition: Option<Expr>,
    },
    Where {
        expr: Expr,
    },
    OrderBy {
        items: Vec<OrderTerm>,
    },
    Limit {
        expr: Expr,
    },
    Offset {
        expr: Expr,
    },
}

/// One `field [asc|desc|$$var]` entry of an `order by` clause.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct OrderItem {
    pub field: String,
    pub field_span: Span,
    pub direction: Option<OrderDirection>,
}

/// One source-ordered static or bounded dynamic ordering term.
#[derive(Debug, Clone, Hash, PartialEq)]
pub enum OrderTerm {
    Column(OrderItem),
    Dynamic {
        variable: VariableRef,
        surface: DynamicInputSurface,
    },
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
    Variable(VariableRef),
}

/// Largest literal pagination value accepted by the PostgreSQL planning layer.
pub(crate) const MAX_PAGINATION_VALUE: i64 = i64::MAX;

/// Parses one query-authored pagination value in PostgreSQL's bigint domain.
pub(crate) fn parse_pagination_value(value: &str) -> Option<u64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .map(|value| value as u64)
}

/// Owns the clause rules (and consumes `clause_list`, `clause`,
/// `order_item`, and `sort_direction` from them).
pub struct Clause;

impl LanguageEntity for Clause {
    const NAME: &'static str = "clause";

    fn register(reg: &mut Registrar<'_>) {
        reg.system(hover_clause_fields);
        reg.system(complete_clause_positions.run_during(bowl::Phase::Complete));
    }
}

/// Answers hover on semantically resolved relation-path segments, terminal
/// columns, and order-by columns inside clauses. The resolution facts own the
/// decision; this service only maps their spans to catalog descriptions.
async fn hover_clause_fields(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    clauses: Query<(Entity, &ResolvedClause), Where<BowlEq<BelongsToFile>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_clause_entity, resolved) = clauses.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let text = resolved
        .paths
        .iter()
        .find_map(|path| {
            path.relations
                .iter()
                .find(|step| step.span.contains(cursor.0))
                .and_then(|step| {
                    describe_relation(catalog, &step.written, step.table, step.relation)
                })
                .or_else(|| match &path.terminal {
                    PathTerminal::Column { span, column, .. } if span.contains(cursor.0) => {
                        describe_column(catalog, *column)
                    }
                    PathTerminal::Column { .. }
                    | PathTerminal::Failed
                    | PathTerminal::OutOfScope => None,
                })
        })
        .or_else(|| {
            resolved.aggregates.iter().find_map(|aggregate| {
                if let Some(relation) = &aggregate.relation
                    && relation.span.contains(cursor.0)
                {
                    return describe_relation(
                        catalog,
                        &relation.written,
                        relation.table,
                        relation.relation,
                    );
                }
                if aggregate
                    .operand_name_span
                    .is_some_and(|span| span.contains(cursor.0))
                    && let Some(operand) = aggregate.operand
                {
                    return describe_column(catalog, operand);
                }
                if aggregate.function_span.contains(cursor.0)
                    && let Some(function) = aggregate.function
                {
                    return Some(format!(
                        "aggregate predicate `{}`: {}{}",
                        function.label(),
                        aggregate
                            .data_type
                            .map_or("unknown", crate::catalog::DataType::as_str),
                        if aggregate.nullable {
                            " (nullable)"
                        } else {
                            ""
                        },
                    ));
                }
                None
            })
        })
        .or_else(|| {
            resolved.order_items.iter().find_map(|item| {
                item.span
                    .contains(cursor.0)
                    .then_some(item.column)
                    .flatten()
                    .and_then(|column| describe_column(catalog, column))
            })
        });

    let Some(text) = text else {
        return;
    };
    emit_hover_candidate(&mut commands, request, priority::FIELD, text);
}

impl LowerStage for Clause {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let fact = if ctx.cst.match_rule(node, Rule::FilterAssignment) {
            let assignment = build_filter_assignment(ctx.cst, ctx.source, node)?;
            ClauseFact::FilterAssignment {
                name: assignment.name,
                name_span: assignment.name_span,
                condition: assignment.condition.map(|condition| *condition),
            }
        } else if ctx.cst.match_rule(node, Rule::WhereClause) {
            ClauseFact::Where {
                expr: clause_expr(ctx, node),
            }
        } else if ctx.cst.match_rule(node, Rule::OrderByClause) {
            ClauseFact::OrderBy {
                items: order_items(ctx.cst, ctx.source, node),
            }
        } else if ctx.cst.match_rule(node, Rule::LimitClause) {
            ClauseFact::Limit {
                expr: clause_expr(ctx, node),
            }
        } else {
            ClauseFact::Offset {
                expr: clause_expr(ctx, node),
            }
        };

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let entity = match ctx.parent {
            Some(parent) => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    node_span(ctx.cst, node),
                    ResolutionScope(ctx.scope.to_string()),
                    fact,
                    ChildOf(parent),
                ))
                .untyped(),
            None => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    node_span(ctx.cst, node),
                    ResolutionScope(ctx.scope.to_string()),
                    fact,
                ))
                .untyped(),
        };
        Some(entity)
    }
}

pub(crate) fn clause_expr(ctx: &LowerCtx<'_>, node: NodeRef) -> Expr {
    match expr_child(ctx.cst, node) {
        Some(expr) => build_expr(ctx.cst, ctx.source, expr),
        None => Expr::Error {
            span: node_span(ctx.cst, node),
        },
    }
}

fn order_items(cst: &CstData, source: &str, node: NodeRef) -> Vec<OrderTerm> {
    cst.children(node)
        .filter(|child| cst.match_rule(*child, Rule::OrderItem))
        .map(|item| {
            if let Some(dynamic) = direct_rule(cst, item, Rule::DynamicInput) {
                return OrderTerm::Dynamic {
                    variable: build_variable_ref(cst, source, dynamic),
                    surface: dynamic_input_surface(cst, dynamic),
                };
            }
            let field_span = direct_rule(cst, item, Rule::QualifiedName)
                .map(|name| node_span(cst, name))
                .unwrap_or_else(|| node_span(cst, item));

            let direction = direct_rule(cst, item, Rule::SortDirection).and_then(|direction| {
                use crate::grammar::lexer::Token;
                use crate::grammar::parser::Node;
                cst.children(direction)
                    .find_map(|child| match cst.get(child) {
                        Node::Token(Token::Asc, _) => Some(OrderDirection::Asc),
                        Node::Token(Token::Desc, _) => Some(OrderDirection::Desc),
                        Node::Rule(Rule::ValueVariable, _) => Some(OrderDirection::Variable(
                            build_variable_ref(cst, source, child),
                        )),
                        _ => None,
                    })
            });

            OrderTerm::Column(OrderItem {
                field: text(source, field_span).to_string(),
                field_span,
                direction,
            })
        })
        .collect()
}

/// Checks one clause against its field's context table during the
/// selection check walk (`field_selection::check_selections`). Path and
/// order-item resolution comes from the clause's [`ResolvedClause`] fact —
/// checks diagnose resolution outcomes, they never re-resolve.
pub(crate) fn check_clause(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    table: crate::catalog::TableId,
    entity: bowl::Entity,
    clause: &ClauseFact,
    span: Span,
) {
    use crate::facts::DiagnosticCode;

    let resolved = ctx.clause_resolutions.get(&entity).copied();
    if let Some(resolved) = resolved {
        crate::entities::field_selection::collect_resolved_clause_tables(
            resolved,
            &mut ctx.observed_tables,
        );
    }
    match clause {
        ClauseFact::FilterAssignment { .. } => {
            crate::entities::policy::check_filter_assignment(ctx, table, entity, clause);
        }
        ClauseFact::Where { expr } => {
            check_dynamic_predicate_placement(
                ctx,
                entity,
                expr,
                DynamicPredicatePlacement::Conjunctive,
            );
            check_predicate_expr(ctx, resolved, table, entity, expr, true);
        }
        ClauseFact::OrderBy { items } => {
            for variable in items.iter().filter_map(|item| match item {
                OrderTerm::Dynamic { variable, .. } => Some(variable),
                OrderTerm::Column(_) => None,
            }) {
                check_dynamic_input_owner(ctx, entity, variable, "order");
            }
            for item in items.iter().filter_map(|item| match item {
                OrderTerm::Column(item) => Some(item),
                OrderTerm::Dynamic { .. } => None,
            }) {
                let resolved_item =
                    resolved.and_then(|resolved| resolved.order_item_at(item.field_span));
                if resolved_item.is_none_or(|item| item.column.is_none()) {
                    let table_name = table_name(ctx, table);
                    ctx.error(
                        entity,
                        item.field_span,
                        DiagnosticCode::FieldNotFound,
                        format!("field `{}` not found on table `{table_name}`", item.field),
                    );
                }
            }
        }
        ClauseFact::Limit { expr } => {
            check_non_negative_integer(ctx, entity, "limit", expr, span);
        }
        ClauseFact::Offset { expr } => {
            check_non_negative_integer(ctx, entity, "offset", expr, span);
        }
    }
}

fn check_dynamic_input_owner(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    entity: bowl::Entity,
    variable: &VariableRef,
    kind: &str,
) {
    use crate::facts::DiagnosticCode;

    if variable.name.is_none() {
        ctx.error(
            entity,
            variable.span,
            DiagnosticCode::InvalidDynamicInput,
            format!("bounded dynamic {kind} inputs must be named top-level query inputs"),
        );
    }
    if ctx.enclosing_fragment.is_some() {
        ctx.error(
            entity,
            variable.span,
            DiagnosticCode::InvalidDynamicInput,
            format!("bounded dynamic {kind} inputs are not supported in fragments"),
        );
    }
}

fn check_dynamic_predicate_placement(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    entity: bowl::Entity,
    expr: &Expr,
    placement: DynamicPredicatePlacement,
) {
    use crate::entities::expression::BinaryOp;
    use crate::facts::DiagnosticCode;

    match expr {
        Expr::DynamicPredicate { variable, span, .. } => {
            check_dynamic_input_owner(ctx, entity, variable, "predicate");
            match placement {
                DynamicPredicatePlacement::Conjunctive => {}
                DynamicPredicatePlacement::InvalidBoolean => ctx.error(
                    entity,
                    *span,
                    DiagnosticCode::InvalidDynamicInput,
                    "bounded dynamic predicates must be the whole predicate or occur in positive `and` position".to_string(),
                ),
                DynamicPredicatePlacement::MissingDynamicInputSurface => ctx.error(
                    entity,
                    *span,
                    DiagnosticCode::InvalidDynamicInput,
                    "bounded dynamic predicates inside `exists` require the nested source to declare a public dynamic input surface".to_string(),
                ),
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let child_placement = match placement {
                DynamicPredicatePlacement::Conjunctive if matches!(op, BinaryOp::And) => {
                    DynamicPredicatePlacement::Conjunctive
                }
                DynamicPredicatePlacement::MissingDynamicInputSurface => {
                    DynamicPredicatePlacement::MissingDynamicInputSurface
                }
                DynamicPredicatePlacement::Conjunctive
                | DynamicPredicatePlacement::InvalidBoolean => {
                    DynamicPredicatePlacement::InvalidBoolean
                }
            };
            check_dynamic_predicate_placement(ctx, entity, lhs, child_placement);
            check_dynamic_predicate_placement(ctx, entity, rhs, child_placement);
        }
        Expr::Unary { operand, .. } | Expr::NullTest { operand, .. } => {
            check_dynamic_predicate_placement(ctx, entity, operand, placement.nested());
        }
        Expr::List { items, .. } => {
            for item in items {
                check_dynamic_predicate_placement(ctx, entity, item, placement.nested());
            }
        }
        Expr::Exists { predicate, .. } => {
            if let Some(predicate) = predicate {
                check_dynamic_predicate_placement(
                    ctx,
                    entity,
                    predicate,
                    DynamicPredicatePlacement::MissingDynamicInputSurface,
                );
            }
        }
        Expr::Aggregate {
            source, operand, ..
        } => {
            check_dynamic_predicate_placement(ctx, entity, source, placement.nested());
            if let Some(operand) = operand {
                check_dynamic_predicate_placement(ctx, entity, operand, placement.nested());
            }
        }
        Expr::Path { .. }
        | Expr::Variable { .. }
        | Expr::PredicateRef { .. }
        | Expr::Literal { .. }
        | Expr::Error { .. } => {}
    }
}

#[derive(Clone, Copy)]
enum DynamicPredicatePlacement {
    Conjunctive,
    InvalidBoolean,
    MissingDynamicInputSurface,
}

impl DynamicPredicatePlacement {
    fn nested(self) -> Self {
        match self {
            Self::MissingDynamicInputSurface => Self::MissingDynamicInputSurface,
            Self::Conjunctive | Self::InvalidBoolean => Self::InvalidBoolean,
        }
    }
}

fn table_name(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    table: crate::catalog::TableId,
) -> String {
    ctx.catalog
        .table_by_id(table)
        .map_or("<unknown>".to_string(), |table| table.name.clone())
}

fn check_predicate_expr(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    table: crate::catalog::TableId,
    entity: bowl::Entity,
    expr: &Expr,
    boolean_position: bool,
) {
    use crate::entities::expression::BinaryOp;
    use crate::facts::DiagnosticCode;

    match expr {
        Expr::Path { .. } => {
            let data_type = resolved_path_type(ctx, resolved, expr);
            if data_type.is_none() {
                let table_name = table_name(ctx, table);
                ctx.error(
                    entity,
                    expr.span(),
                    DiagnosticCode::FieldNotFound,
                    format!("field `{expr}` not found on table `{table_name}`"),
                );
            } else if boolean_position {
                ctx.error(
                    entity,
                    expr.span(),
                    DiagnosticCode::PredicateTypeMismatch,
                    format!("bare field `{expr}` is not a predicate; compare or test it"),
                );
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            match op {
                BinaryOp::Comparison(op) => {
                    check_aggregate_comparison_operator(ctx, resolved, entity, lhs, *op, rhs);
                    check_aggregate_comparison_path(ctx, resolved, entity, lhs, rhs);
                    check_binary_predicate_types(ctx, resolved, entity, lhs, *op, rhs);
                }
                BinaryOp::Variable(operator) => {
                    check_operator_variable(ctx, resolved, entity, lhs, rhs, operator);
                }
                BinaryOp::In | BinaryOp::NotIn => {
                    check_membership_types(ctx, resolved, entity, lhs, rhs);
                }
                BinaryOp::And | BinaryOp::Or => {}
            }
            let child_boolean = matches!(op, BinaryOp::And | BinaryOp::Or);
            check_predicate_expr(ctx, resolved, table, entity, lhs, child_boolean);
            check_predicate_expr(ctx, resolved, table, entity, rhs, child_boolean);
        }
        Expr::Unary { operand, .. } => {
            check_predicate_expr(ctx, resolved, table, entity, operand, true);
        }
        Expr::NullTest { operand, .. } => {
            if !matches!(operand.as_ref(), Expr::Path { .. }) {
                ctx.error(
                    entity,
                    operand.span(),
                    DiagnosticCode::PredicateTypeMismatch,
                    "null-test operand must be a field path".to_string(),
                );
            }
            check_predicate_expr(ctx, resolved, table, entity, operand, false);
        }
        Expr::List { items, .. } => {
            if boolean_position {
                ctx.error(
                    entity,
                    expr.span(),
                    DiagnosticCode::PredicateTypeMismatch,
                    "collection literal is not a boolean predicate".to_string(),
                );
            }
            for item in items {
                check_predicate_expr(ctx, resolved, table, entity, item, false);
            }
        }
        Expr::Exists {
            filters,
            predicate,
            span,
            ..
        } => {
            let existence = resolved.and_then(|resolved| resolved.existence_at(*span));
            if let Some(existence) = existence {
                for problem in &existence.problems {
                    use crate::resolution::ExistenceProblemKind;
                    let (code, message) = match &problem.kind {
                        ExistenceProblemKind::SourceMustBeCollection => (
                            DiagnosticCode::PredicateTypeMismatch,
                            "`exists` source must be a to-many relation or table".to_string(),
                        ),
                        ExistenceProblemKind::FieldNotFound(reference) => (
                            DiagnosticCode::FieldNotFound,
                            format!("field `{reference}` not found"),
                        ),
                        ExistenceProblemKind::AmbiguousRelation {
                            reference,
                            candidates,
                        } => (
                            DiagnosticCode::AmbiguousRelation,
                            format!(
                                "relation `{reference}` is ambiguous; candidates: {}",
                                candidates.join(", ")
                            ),
                        ),
                        ExistenceProblemKind::TableNotFound(reference) => (
                            DiagnosticCode::TableNotFound,
                            format!("table `{reference}` not found"),
                        ),
                        ExistenceProblemKind::AmbiguousTable {
                            reference,
                            candidates,
                        } => (
                            DiagnosticCode::AmbiguousTable,
                            format!(
                                "table `{reference}` is ambiguous; candidates: {}",
                                candidates
                                    .iter()
                                    .map(|candidate| {
                                        format!("{}::{}", candidate.schema, candidate.table)
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        ),
                    };
                    ctx.error(entity, problem.span, code, message);
                }
            }
            if let Some(predicate) = predicate {
                let nested_table = existence.and_then(|existence| {
                    existence.source.as_ref().map(|source| match source {
                        crate::resolution::ResolvedExistenceSource::Relation(relation) => {
                            relation.table
                        }
                        crate::resolution::ResolvedExistenceSource::Table(table) => *table,
                    })
                });
                if let Some(nested_table) = nested_table {
                    crate::entities::policy::check_exists_filter_assignments(
                        ctx,
                        nested_table,
                        entity,
                        filters,
                    );
                }
                check_predicate_expr(
                    ctx,
                    resolved,
                    nested_table.unwrap_or(table),
                    entity,
                    predicate,
                    true,
                );
            } else if let Some(nested_table) = existence.and_then(|existence| {
                existence.source.as_ref().map(|source| match source {
                    crate::resolution::ResolvedExistenceSource::Relation(relation) => {
                        relation.table
                    }
                    crate::resolution::ResolvedExistenceSource::Table(table) => *table,
                })
            }) {
                crate::entities::policy::check_exists_filter_assignments(
                    ctx,
                    nested_table,
                    entity,
                    filters,
                );
            }
        }
        Expr::Aggregate { .. } => {
            let Some(aggregate) = resolved.and_then(|resolved| resolved.aggregate_at(expr.span()))
            else {
                return;
            };
            for problem in &aggregate.problems {
                ctx.error(
                    entity,
                    problem.span,
                    problem.kind.code(),
                    problem.kind.message(),
                );
            }
            if boolean_position
                && let Some(function) = aggregate.function
                && function != crate::entities::aggregate::AggregateFunction::Exists
            {
                let problem = crate::entities::aggregate::AggregateProblemKind::PredicateAggregateMustBeBoolean {
                    function,
                };
                ctx.error(entity, expr.span(), problem.code(), problem.message());
            }
        }
        Expr::PredicateRef { name, span } => {
            ctx.error(
                entity,
                *span,
                DiagnosticCode::PredicateTypeMismatch,
                format!("condition `{name}` may only be referenced from a filter definition"),
            );
        }
        Expr::Literal { value, .. } => {
            if boolean_position
                && !matches!(value, crate::entities::expression::LiteralValue::Bool(_))
            {
                ctx.error(
                    entity,
                    expr.span(),
                    DiagnosticCode::PredicateTypeMismatch,
                    "predicate atom must have type boolean".to_string(),
                );
            }
        }
        Expr::Variable { .. } | Expr::DynamicPredicate { .. } | Expr::Error { .. } => {}
    }
}

fn check_membership_types(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    entity: bowl::Entity,
    lhs: &Expr,
    rhs: &Expr,
) {
    use crate::facts::DiagnosticCode;

    if !matches!(lhs, Expr::Path { .. }) {
        ctx.error(
            entity,
            lhs.span(),
            DiagnosticCode::PredicateTypeMismatch,
            "membership left operand must be a field path".to_string(),
        );
        return;
    }
    let Some((_, capabilities)) = resolved_expr_semantics(ctx, resolved, lhs) else {
        return;
    };
    let Expr::List { items, .. } = rhs else {
        if !matches!(rhs, Expr::Variable { .. }) {
            ctx.error(
                entity,
                rhs.span(),
                DiagnosticCode::PredicateTypeMismatch,
                "membership expects a list literal or collection variable".to_string(),
            );
        }
        return;
    };
    for item in items {
        if let Expr::Literal { value, span } = item {
            check_literal_for_capabilities(ctx, entity, &capabilities, lhs, value, *span);
        } else {
            ctx.error(
                entity,
                item.span(),
                DiagnosticCode::PredicateTypeMismatch,
                "membership list items must be literals".to_string(),
            );
        }
    }
}

fn check_literal_for_capabilities(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    entity: bowl::Entity,
    capabilities: &crate::catalog::TypeCapabilities,
    path: &Expr,
    value: &crate::entities::expression::LiteralValue,
    span: Span,
) {
    use crate::catalog::LiteralKind;
    use crate::entities::expression::LiteralValue;
    use crate::facts::DiagnosticCode;

    let (actual, raw_value) = match value {
        LiteralValue::String(value) => (LiteralKind::String, value.as_str()),
        LiteralValue::Number(value) => (LiteralKind::Number, value.as_str()),
        LiteralValue::Bool(true) => (LiteralKind::Boolean, "true"),
        LiteralValue::Bool(false) => (LiteralKind::Boolean, "false"),
        LiteralValue::Null => return,
    };
    if !capabilities.literals.accepts(actual, raw_value) {
        ctx.error(
            entity,
            span,
            DiagnosticCode::PredicateTypeMismatch,
            format!(
                "field `{path}` expects {} but membership uses {}",
                capabilities.literals.description,
                actual.as_str()
            ),
        );
    }
}

fn check_aggregate_comparison_path(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    entity: bowl::Entity,
    lhs: &Expr,
    rhs: &Expr,
) {
    use crate::facts::DiagnosticCode;

    let path = match (lhs, rhs) {
        (Expr::Aggregate { .. }, path @ Expr::Path { .. })
        | (path @ Expr::Path { .. }, Expr::Aggregate { .. }) => path,
        _ => return,
    };
    let Expr::Path {
        anchor, segments, ..
    } = path
    else {
        return;
    };
    if segments.len() == 1 && matches!(anchor, PathAnchor::Current | PathAnchor::Root) {
        return;
    }
    if resolved_path_type(ctx, resolved, path).is_none() {
        return;
    }
    ctx.error(
        entity,
        path.span(),
        DiagnosticCode::ClauseValueTypeMismatch,
        "aggregate comparisons only support direct `.`- or `~`-anchored scalar path operands"
            .to_string(),
    );
}

fn check_aggregate_comparison_operator(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    entity: bowl::Entity,
    lhs: &Expr,
    op: crate::entities::expression::ComparisonOp,
    rhs: &Expr,
) {
    use crate::facts::DiagnosticCode;

    let aggregate = match (lhs, rhs) {
        (aggregate @ Expr::Aggregate { .. }, _) | (_, aggregate @ Expr::Aggregate { .. }) => {
            aggregate
        }
        _ => return,
    };
    let Some((data_type, capabilities)) = resolved_expr_semantics(ctx, resolved, aggregate) else {
        return;
    };
    if !capabilities.supports(op) {
        ctx.error(
            entity,
            aggregate.span(),
            DiagnosticCode::ClauseValueTypeMismatch,
            format!(
                "aggregate predicate of type {} does not support operator `{}`",
                data_type.as_str(),
                op.as_str()
            ),
        );
    }
}

fn check_operator_variable(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    entity: bowl::Entity,
    lhs: &Expr,
    rhs: &Expr,
    operator: &VariableRef,
) {
    use crate::facts::DiagnosticCode;

    let path = match (lhs, rhs) {
        (path @ (Expr::Path { .. } | Expr::Aggregate { .. }), _)
        | (_, path @ (Expr::Path { .. } | Expr::Aggregate { .. })) => path,
        _ => return,
    };
    if matches!(path, Expr::Aggregate { .. }) {
        ctx.error(
            entity,
            operator.span,
            DiagnosticCode::ClauseValueTypeMismatch,
            "operator variables are not supported for aggregate predicates".to_string(),
        );
        return;
    }
    let Some((data_type, capabilities)) = resolved_expr_semantics(ctx, resolved, path) else {
        return;
    };
    let Some(allowed) = &operator.operators else {
        return;
    };
    let compares_null = matches!(
        (lhs, rhs),
        (
            Expr::Literal {
                value: crate::entities::expression::LiteralValue::Null,
                ..
            },
            _
        ) | (
            _,
            Expr::Literal {
                value: crate::entities::expression::LiteralValue::Null,
                ..
            }
        )
    );
    if compares_null
        && allowed.iter().any(|operator| {
            !matches!(
                operator,
                crate::entities::expression::ComparisonOp::Eq
                    | crate::entities::expression::ComparisonOp::Ne
            )
        })
    {
        ctx.error(
            entity,
            operator.span,
            DiagnosticCode::PredicateTypeMismatch,
            "operator-variable comparisons against `null` only support `==` and `!=`".to_string(),
        );
    }
    for op in allowed {
        if !capabilities.supports(*op) {
            ctx.error(
                entity,
                operator.span,
                DiagnosticCode::ClauseValueTypeMismatch,
                format!(
                    "clause `operator` expects an operator valid for {}",
                    data_type.as_str()
                ),
            );
        }
    }
}

fn check_binary_predicate_types(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    entity: bowl::Entity,
    lhs: &Expr,
    op: crate::entities::expression::ComparisonOp,
    rhs: &Expr,
) {
    use crate::catalog::{DataType, LiteralKind};
    use crate::entities::expression::{ComparisonOp, LiteralValue};
    use crate::facts::DiagnosticCode;

    let (path, literal, literal_span) = match (lhs, rhs) {
        (path @ (Expr::Path { .. } | Expr::Aggregate { .. }), Expr::Literal { value, span }) => {
            (path, value, *span)
        }
        (Expr::Literal { value, span }, path @ (Expr::Path { .. } | Expr::Aggregate { .. })) => {
            (path, value, *span)
        }
        _ => return,
    };
    let Some((_, capabilities)) = resolved_expr_semantics(ctx, resolved, path) else {
        return;
    };
    if matches!(path, Expr::Aggregate { .. }) && !capabilities.supports(op) {
        return;
    }
    let (actual, raw_value) = match literal {
        LiteralValue::String(value) => (LiteralKind::String, value.as_str()),
        LiteralValue::Number(value) => (LiteralKind::Number, value.as_str()),
        LiteralValue::Bool(true) => (LiteralKind::Boolean, "true"),
        LiteralValue::Bool(false) => (LiteralKind::Boolean, "false"),
        LiteralValue::Null => {
            if !matches!(op, ComparisonOp::Eq | ComparisonOp::Ne) {
                ctx.error(
                    entity,
                    literal_span,
                    DiagnosticCode::PredicateTypeMismatch,
                    format!(
                        "comparison with `null` does not support operator `{}`; use `==` or `!=`",
                        op.as_str()
                    ),
                );
            }
            return;
        }
    };
    if op == ComparisonOp::Like && !capabilities.supports(ComparisonOp::Like) {
        let text = crate::catalog::Catalog::builtin_capabilities(DataType::Text);
        ctx.error(
            entity,
            literal_span,
            DiagnosticCode::PredicateTypeMismatch,
            format!(
                "field `{path}` expects {} but predicate uses {}",
                text.literals.description,
                actual.as_str()
            ),
        );
        return;
    }
    if !capabilities.literals.accepts(actual, raw_value) {
        ctx.error(
            entity,
            literal_span,
            DiagnosticCode::PredicateTypeMismatch,
            format!(
                "field `{path}` expects {} but predicate uses {}",
                capabilities.literals.description,
                actual.as_str()
            ),
        );
    }
}

/// The terminal column type of a path, read from the clause's
/// resolution fact.
fn resolved_path_type(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    path: &Expr,
) -> Option<crate::catalog::DataType> {
    let column = resolved?.path_at(path.span())?.terminal.column()?;
    ctx.catalog
        .column_by_id(column)
        .map(|column| ctx.catalog.data_type_for_column(column.id))
}

fn resolved_expr_semantics<'a>(
    ctx: &crate::entities::field_selection::CheckCtx<'a, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    expr: &Expr,
) -> Option<(
    crate::catalog::DataType,
    Cow<'a, crate::catalog::TypeCapabilities>,
)> {
    match expr {
        Expr::Path { .. } => {
            let column = resolved?.path_at(expr.span())?.terminal.column()?;
            let data_type = ctx.catalog.type_for_column(column)?;
            Some((data_type.data_type, Cow::Borrowed(&data_type.capabilities)))
        }
        Expr::Aggregate { .. } => {
            let data_type = resolved_expr_type(ctx, resolved, expr)?;
            Some((
                data_type,
                Cow::Owned(crate::catalog::Catalog::builtin_capabilities(data_type)),
            ))
        }
        _ => None,
    }
}

fn resolved_expr_type(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    resolved: Option<&crate::resolution::ResolvedClause>,
    expr: &Expr,
) -> Option<crate::catalog::DataType> {
    match expr {
        Expr::Path { .. } => resolved_path_type(ctx, resolved, expr),
        Expr::Aggregate { .. } => {
            let aggregate = resolved?.aggregate_at(expr.span())?;
            if !aggregate.is_valid() {
                return None;
            }
            aggregate.data_type
        }
        Expr::Binary { .. }
        | Expr::Unary { .. }
        | Expr::NullTest { .. }
        | Expr::List { .. }
        | Expr::Exists { .. }
        | Expr::Literal { .. }
        | Expr::Variable { .. }
        | Expr::DynamicPredicate { .. }
        | Expr::PredicateRef { .. }
        | Expr::Error { .. } => None,
    }
}

fn check_non_negative_integer(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    entity: bowl::Entity,
    clause: &str,
    expr: &Expr,
    span: Span,
) {
    use crate::entities::expression::LiteralValue;
    use crate::facts::DiagnosticCode;

    if matches!(expr, Expr::Aggregate { .. } | Expr::Exists { .. }) {
        ctx.error(
            entity,
            expr.span(),
            DiagnosticCode::ClauseValueTypeMismatch,
            format!("clause `{clause}` does not accept aggregate predicate expressions"),
        );
        return;
    }
    let valid = matches!(
        expr,
        Expr::Literal { value: LiteralValue::Number(value), .. }
            if parse_pagination_value(value).is_some()
    ) || matches!(expr, Expr::Variable { .. });
    if !valid {
        ctx.error(
            entity,
            span,
            DiagnosticCode::ClauseValueTypeMismatch,
            format!(
                "clause `{clause}` expects a non-negative integer no greater than {}",
                MAX_PAGINATION_VALUE,
            ),
        );
    }
}

impl FormatStage for Clause {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        if formatter.rule(node) == Some(Rule::FilterAssignment) {
            formatter.filter_assignment(node);
        } else if formatter.rule(node) == Some(Rule::WhereClause) {
            formatter.write_str("where ");
            if let Some(value) = formatter.direct_value_rule(node) {
                formatter.expr(value);
            }
        } else if formatter.rule(node) == Some(Rule::OrderByClause) {
            formatter.write_str("order by ");
            for (idx, item) in formatter
                .direct_rules(node, Rule::OrderItem)
                .into_iter()
                .enumerate()
            {
                if idx > 0 {
                    formatter.write_str(", ");
                }
                if formatter.direct_rule(item, Rule::DynamicInput).is_some() {
                    formatter.write_node_text(item);
                } else {
                    formatter.order_item(item);
                }
            }
        } else if formatter.rule(node) == Some(Rule::LimitClause) {
            formatter.write_str("limit ");
            if let Some(value) = formatter.direct_value_rule(node) {
                formatter.expr(value);
            }
        } else {
            formatter.write_str("offset ");
            if let Some(value) = formatter.direct_value_rule(node) {
                formatter.expr(value);
            }
        }
    }
}

/// Contributes scope anchors and columns inside `where` predicates and
/// columns inside `order by` items. Clause keywords and comparison
/// operators come from the grammar layer.
async fn complete_clause_positions(
    requests: Query<
        (Entity, &crate::service::completion::CompletionContext),
        With<crate::service::completion::CompletionRequest>,
    >,
    catalog: Query<(Entity, &crate::catalog::CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    use crate::service::completion::{
        CompletionItem, CompletionKind, CompletionSite, emit_completion_candidate,
    };

    let (request, context) = requests.item();
    let (_, snapshot) = catalog.item();

    let Some(table) = context.table else {
        return;
    };
    let in_where = context.site == CompletionSite::WhereExpr;
    let in_order_by = context.site == CompletionSite::OrderBy;
    if !in_where && !in_order_by {
        return;
    }

    let mut items = Vec::new();
    let mut push = |item: CompletionItem| items.push(item);

    if in_where {
        for (anchor, detail) in [
            (".", "current scope"),
            ("..", "parent scope"),
            ("~", "root scope"),
        ] {
            push(CompletionItem {
                label: anchor.to_string(),
                kind: CompletionKind::Scope,
                detail: Some(detail.to_string()),
                documentation: None,
                insert_text: None,
            });
        }
    }

    for column in snapshot.catalog().columns_for_table(table) {
        push(CompletionItem {
            label: column.name.clone(),
            kind: CompletionKind::Column,
            detail: Some(
                snapshot
                    .catalog()
                    .data_type_for_column(column.id)
                    .as_str()
                    .to_string(),
            ),
            documentation: column.description.clone(),
            insert_text: None,
        });
    }

    emit_completion_candidate(&mut commands, request, items);
}
