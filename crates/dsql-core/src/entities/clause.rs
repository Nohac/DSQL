//! Clause entity: the `(where ... order by ... limit ... offset ...)`
//! clause list attached to a field selection.
//!
//! One entity covers all four clause kinds — they share shape, checks, and
//! planning surface; [`ClauseFact`] branches where they differ.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query, SystemExt, With};

use crate::entities::expression::{Expr, VariableRef, build_expr, build_variable_ref, expr_child};
use crate::entities::{direct_rule, node_span, text};
use crate::entity::{CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage};
use crate::format::CstFormatter;
use crate::facts::{BelongsToFile, NodeKey, ParentKey, Span};
use crate::grammar::parser::{CstData, NodeRef, Rule};

/// One clause, lowered from `where_clause` / `order_by_clause` /
/// `limit_clause` / `offset_clause`. `ParentKey` links it to the field
/// selection it constrains.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub enum ClauseFact {
    Where { expr: Expr },
    OrderBy { items: Vec<OrderItem> },
    Limit { expr: Expr },
    Offset { expr: Expr },
}

/// One `field [asc|desc|$$var]` entry of an `order by` clause.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct OrderItem {
    pub field: String,
    pub field_span: Span,
    pub direction: Option<OrderDirection>,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
    Variable(VariableRef),
}

/// Owns the clause rules (and consumes `clause_list`, `clause`,
/// `order_item`, and `sort_direction` from them).
pub struct Clause;

impl LanguageEntity for Clause {
    const NAME: &'static str = "clause";

    async fn register(_bowl: &Bowl) {
        // Clause type checks against the catalog land in phase 6.
    }
}

impl LowerStage for Clause {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
        let fact = if ctx.cst.match_rule(node, Rule::WhereClause) {
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

        let entity = commands.insert((
            DerivedFrom::new(ctx.file),
            BelongsToFile(ctx.file),
            key,
            node_span(ctx.cst, node),
            fact,
        ));
        if let Some(parent) = ctx.parent {
            commands.entity(entity).insert(ParentKey(parent));
        }
    }
}

fn clause_expr(ctx: &LowerCtx<'_>, node: NodeRef) -> Expr {
    match expr_child(ctx.cst, node) {
        Some(expr) => build_expr(ctx.cst, ctx.source, expr),
        None => Expr::Error {
            span: node_span(ctx.cst, node),
        },
    }
}

fn order_items(cst: &CstData, source: &str, node: NodeRef) -> Vec<OrderItem> {
    cst.children(node)
        .filter(|child| cst.match_rule(*child, Rule::OrderItem))
        .map(|item| {
            let field_span = direct_rule(cst, item, Rule::QualifiedName)
                .map(|name| node_span(cst, name))
                .unwrap_or_else(|| node_span(cst, item));

            let direction = direct_rule(cst, item, Rule::SortDirection).and_then(|direction| {
                use crate::grammar::lexer::Token;
                use crate::grammar::parser::Node;
                cst.children(direction).find_map(|child| match cst.get(child) {
                    Node::Token(Token::Asc, _) => Some(OrderDirection::Asc),
                    Node::Token(Token::Desc, _) => Some(OrderDirection::Desc),
                    Node::Rule(Rule::ValueVariable, _) => Some(OrderDirection::Variable(
                        build_variable_ref(cst, source, child),
                    )),
                    _ => None,
                })
            });

            OrderItem {
                field: text(source, field_span).to_string(),
                field_span,
                direction,
            }
        })
        .collect()
}

/// Checks one clause against its field's context table during the selection
/// check walk (`field_selection::check_selections`). Clause semantics live
/// here with the entity that owns the rules.
pub(crate) fn check_clause(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    root_table: crate::catalog::TableId,
    table: crate::catalog::TableId,
    entity: bowl::Entity,
    clause: &ClauseFact,
    span: Span,
) {
    use crate::catalog::{FieldCheckResult, FieldRef, TableRef};
    use crate::facts::DiagnosticCode;

    match clause {
        ClauseFact::Where { expr } => {
            check_predicate_expr(ctx, root_table, table, entity, expr);
        }
        ClauseFact::OrderBy { items } => {
            for item in items {
                let reference = FieldRef {
                    target: TableRef::parse(&item.field),
                    selector: None,
                };
                if !matches!(
                    ctx.catalog.check_field_ref(table, reference),
                    FieldCheckResult::Column(_)
                ) {
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

fn table_name(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    table: crate::catalog::TableId,
) -> String {
    ctx.catalog
        .table_by_id(table)
        .map_or("<unknown>".to_string(), |table| table.name.clone())
}

/// Predicate checks: paths must resolve, and path-vs-literal comparisons
/// must agree with the column's type.
fn check_predicate_expr(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    root_table: crate::catalog::TableId,
    table: crate::catalog::TableId,
    entity: bowl::Entity,
    expr: &Expr,
) {
    use crate::entities::expression::BinaryOp;
    use crate::facts::DiagnosticCode;

    match expr {
        Expr::Path { .. } => {
            if resolve_predicate_path(ctx, root_table, table, expr).is_none() {
                let table_name = table_name(ctx, table);
                ctx.error(
                    entity,
                    expr.span(),
                    DiagnosticCode::FieldNotFound,
                    format!(
                        "field `{}` not found on table `{table_name}`",
                        expr
                    ),
                );
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            match op {
                BinaryOp::Comparison(op) => {
                    check_binary_predicate_types(ctx, root_table, table, entity, lhs, *op, rhs);
                }
                BinaryOp::Variable(operator) => {
                    check_operator_variable(ctx, root_table, table, entity, lhs, rhs, operator);
                }
                BinaryOp::And | BinaryOp::Or => {}
            }
            check_predicate_expr(ctx, root_table, table, entity, lhs);
            check_predicate_expr(ctx, root_table, table, entity, rhs);
        }
        Expr::Literal { .. } | Expr::Variable { .. } | Expr::Error { .. } => {}
    }
}

fn check_operator_variable(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    root_table: crate::catalog::TableId,
    table: crate::catalog::TableId,
    entity: bowl::Entity,
    lhs: &Expr,
    rhs: &Expr,
    operator: &VariableRef,
) {
    use crate::facts::DiagnosticCode;

    let path = match (lhs, rhs) {
        (path @ Expr::Path { .. }, _) | (_, path @ Expr::Path { .. }) => path,
        _ => return,
    };
    let Some(data_type) = resolve_predicate_path(ctx, root_table, table, path) else {
        return;
    };
    let Some(allowed) = &operator.operators else {
        return;
    };
    for op in allowed {
        if !data_type.operator_ops().contains(op) {
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
    root_table: crate::catalog::TableId,
    table: crate::catalog::TableId,
    entity: bowl::Entity,
    lhs: &Expr,
    op: crate::entities::expression::ComparisonOp,
    rhs: &Expr,
) {
    use crate::catalog::{DataType, LiteralKind};
    use crate::entities::expression::{ComparisonOp, LiteralValue};
    use crate::facts::DiagnosticCode;

    let (path, literal, literal_span) = match (lhs, rhs) {
        (path @ Expr::Path { .. }, Expr::Literal { value, span }) => (path, value, *span),
        (Expr::Literal { value, span }, path @ Expr::Path { .. }) => (path, value, *span),
        _ => return,
    };
    let Some(data_type) = resolve_predicate_path(ctx, root_table, table, path) else {
        return;
    };
    let (actual, raw_value) = match literal {
        LiteralValue::String(value) => (LiteralKind::String, value.as_str()),
        LiteralValue::Number(value) => (LiteralKind::Number, value.as_str()),
        LiteralValue::Bool(true) => (LiteralKind::Boolean, "true"),
        LiteralValue::Bool(false) => (LiteralKind::Boolean, "false"),
        LiteralValue::Null => return,
    };
    if op == ComparisonOp::Like && data_type != DataType::Text {
        ctx.error(
            entity,
            literal_span,
            DiagnosticCode::PredicateTypeMismatch,
            format!(
                "field `{path}` expects {} but predicate uses {}",
                DataType::Text.expected_literal_description(),
                actual.as_str()
            ),
        );
        return;
    }
    if !data_type.accepts_literal_value(actual, raw_value) {
        ctx.error(
            entity,
            literal_span,
            DiagnosticCode::PredicateTypeMismatch,
            format!(
                "field `{path}` expects {} but predicate uses {}",
                data_type.expected_literal_description(),
                actual.as_str()
            ),
        );
    }
}

/// Resolves a scoped path to the column it names, stepping through relation
/// segments. Parent scope (`..`) is not resolvable at check time.
fn resolve_predicate_path(
    ctx: &crate::entities::field_selection::CheckCtx<'_, '_>,
    root_table: crate::catalog::TableId,
    table: crate::catalog::TableId,
    path: &Expr,
) -> Option<crate::catalog::DataType> {
    use crate::catalog::{FieldCheckResult, FieldRef, TableRef};
    use crate::entities::expression::PathAnchor;

    let Expr::Path {
        anchor, segments, ..
    } = path
    else {
        return None;
    };
    let mut current_table = match anchor {
        PathAnchor::Current => table,
        PathAnchor::Root => root_table,
        PathAnchor::Parent => return None,
    };
    let (last, relations) = segments.split_last()?;
    for segment in relations {
        let reference = FieldRef {
            target: TableRef::parse(&segment.name),
            selector: segment.relation_path.as_deref(),
        };
        let FieldCheckResult::Relation(relation) = ctx.catalog.check_field_ref(current_table, reference)
        else {
            return None;
        };
        current_table = relation.table.id;
    }
    let reference = FieldRef {
        target: TableRef::parse(&last.name),
        selector: last.relation_path.as_deref(),
    };
    let FieldCheckResult::Column(column) = ctx.catalog.check_field_ref(current_table, reference)
    else {
        return None;
    };
    Some(column.data_type)
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

    let valid = matches!(
        expr,
        Expr::Literal { value: LiteralValue::Number(value), .. } if value.parse::<u64>().is_ok()
    ) || matches!(expr, Expr::Variable { .. });
    if !valid {
        ctx.error(
            entity,
            span,
            DiagnosticCode::ClauseValueTypeMismatch,
            format!("clause `{clause}` expects a non-negative integer"),
        );
    }
}


impl FormatStage for Clause {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        if formatter.rule(node) == Some(Rule::WhereClause) {
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
                formatter.order_item(item);
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


impl HoverStage for Clause {
    /// Clause keywords carry no hover content of their own; the paths and
    /// variables inside them answer through their entities.
    async fn register_hover(_bowl: &Bowl) {}
}


impl CompletionStage for Clause {
    async fn register_completions(bowl: &Bowl) {
        bowl.add_system(complete_clause_positions.run_during(bowl::Phase::Complete))
            .await;
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
    mut commands: Commands,
) {
    use crate::service::completion::{CompletionCandidate, CompletionItem, CompletionKind, CompletionSite};

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
                insert_text: None,
            });
        }
    }

    for column in snapshot.catalog().columns_for_table(table) {
        push(CompletionItem {
            label: column.name.clone(),
            kind: CompletionKind::Column,
            detail: Some(column.data_type.as_str().to_string()),
            insert_text: None,
        });
    }

    if !items.is_empty() {
        commands.insert((
            DerivedFrom::new(request),
            crate::service::hover::RequestKey(request),
            CompletionCandidate { items },
        ));
    }
}
