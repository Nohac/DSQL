//! Clause entity: the `(where ... order by ... limit ... offset ...)`
//! clause list attached to a field selection.
//!
//! One entity covers all four clause kinds — they share shape, checks, and
//! planning surface; [`ClauseFact`] branches where they differ.

use bowl::{Bowl, Commands, Component, DerivedFrom};

use crate::entities::expression::{Expr, VariableRef, build_expr, build_variable_ref, expr_child};
use crate::entities::{direct_rule, node_span, text};
use crate::entity::{LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{BelongsToFile, NodeKey, ParentKey, Span};
use crate::grammar::parser::{CstData, NodeRef, Rule};

/// One clause, lowered from `where_clause` / `order_by_clause` /
/// `limit_clause` / `offset_clause`. `ParentKey` links it to the field
/// selection it constrains.
#[derive(Component, Debug, Hash)]
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

    async fn register(_db: &Bowl) {
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

        match ctx.parent {
            Some(parent) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                ParentKey(parent),
                node_span(ctx.cst, node),
                fact,
            )),
            None => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                node_span(ctx.cst, node),
                fact,
            )),
        };
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
