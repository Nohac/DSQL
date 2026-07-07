//! The shared lowering walk and rule-ownership dispatch.
//!
//! [`generate_ast`] is the one generic CST walk: it visits every rule node
//! once and hands it to the entity that owns the rule. Ownership lives in
//! `lower_rule` — an exhaustive `match`, so adding a rule to `dsql.llw`
//! fails to compile here until an entity claims it or it is explicitly
//! listed as structural.

use bowl::{Commands, Entity, Query};

use crate::entity::{LowerCtx, LowerStage};
use crate::facts::{NodeKey, Span};
use crate::grammar::lexer::Token;
use crate::grammar::parser::{CstData, Node, NodeRef, Rule};
use super::clause::Clause;
use super::definition::Definition;
use super::directive::Directive;
use super::document::{Document, ParsedFile};
use super::expression::Expression;
use super::field_selection::FieldSelection;
use super::fragment_spread::FragmentSpread;
use super::variable::Variable;

fn lower_rule(ctx: &LowerCtx<'_>, rule: Rule, node: NodeRef, commands: &mut Commands) {
    match rule {
        Rule::Document => Document::lower(ctx, node, commands),
        Rule::QueryDef | Rule::FragmentDef => Definition::lower(ctx, node, commands),
        Rule::FieldSelection => FieldSelection::lower(ctx, node, commands),
        Rule::FragmentSpread => FragmentSpread::lower(ctx, node, commands),
        Rule::WhereClause | Rule::OrderByClause | Rule::LimitClause | Rule::OffsetClause => {
            Clause::lower(ctx, node, commands)
        }
        Rule::Directive => Directive::lower(ctx, node, commands),
        Rule::ValueVariable | Rule::OperatorVariable => Variable::lower(ctx, node, commands),
        // Expression rules lower as part of the clause/directive facts that
        // contain them (expression::build_expr); the claim is a no-op.
        Rule::Expr
        | Rule::BinaryExpr
        | Rule::Literal
        | Rule::BinaryOperator
        | Rule::ComparisonOperator
        | Rule::ScopedPath
        | Rule::ScopedPathSegment => Expression::lower(ctx, node, commands),

        // Consumed by FieldSelection lowering from the field_selection node.
        Rule::FieldSelectionTail | Rule::FieldSuffix => {}
        // Consumed by Clause lowering from the clause nodes.
        Rule::Clause | Rule::ClauseList | Rule::OrderItem | Rule::SortDirection => {}
        // Consumed by Directive lowering from the directive node.
        Rule::DirectiveName
        | Rule::DirectiveNamespace
        | Rule::DirectiveMember
        | Rule::DirectiveArgument => {}
        // Consumed by Definition (fragment targets), FieldSelection
        // (relation refs), and Clause (order items) lowerings.
        Rule::QualifiedName | Rule::RelationRef => {}

        // Structural rules: consumed by the entities owning their
        // ancestors, never owned themselves.
        Rule::Definition | Rule::Selection | Rule::SelectionSet => {}

        // Error recovery nodes surface through parse diagnostics, not
        // through lowering.
        Rule::Error => {}
    }
}

/// The one generic lowering walk: one invocation per parsed file, visiting
/// every CST rule node once and dispatching to the owning entity.
pub async fn generate_ast(query: Query<(Entity, &ParsedFile)>, mut commands: Commands) {
    let (file, parsed) = query.item();

    let ctx = LowerCtx {
        cst: &parsed.cst,
        source: &parsed.source,
        file,
        parent: None,
    };
    walk(&ctx, NodeRef::ROOT, &mut commands);
}

fn walk(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
    if let Node::Rule(rule, _) = ctx.cst.get(node) {
        lower_rule(ctx, rule, node, commands);

        // Definitions and field selections form the selection tree; spreads,
        // clauses, and directives are attachment points for the facts nested
        // inside them (directives on spreads, variables in clauses). Descend
        // with this node as the parent so nested facts carry their position
        // as a `ParentKey`.
        if matches!(
            rule,
            Rule::QueryDef
                | Rule::FragmentDef
                | Rule::FieldSelection
                | Rule::FragmentSpread
                | Rule::WhereClause
                | Rule::OrderByClause
                | Rule::LimitClause
                | Rule::OffsetClause
                | Rule::Directive
        ) {
            let scoped = LowerCtx {
                cst: ctx.cst,
                source: ctx.source,
                file: ctx.file,
                parent: Some(NodeKey {
                    file: ctx.file,
                    node: node.0,
                }),
            };
            for child in ctx.cst.children(node) {
                walk(&scoped, child, commands);
            }
            return;
        }
    }

    for child in ctx.cst.children(node) {
        walk(ctx, child, commands);
    }
}

// CST helpers shared by entity lowerings. All operate on direct children
// only; recursive extraction belongs to the entity owning the nested rule.

/// Span of the first direct child token of `node` matching `token`.
pub(crate) fn direct_token(cst: &CstData, node: NodeRef, token: Token) -> Option<Span> {
    cst.children(node)
        .find_map(|child| cst.match_token(child, token).map(Span::from))
}

/// First direct child of `node` that is a `rule` node.
pub(crate) fn direct_rule(cst: &CstData, node: NodeRef, rule: Rule) -> Option<NodeRef> {
    cst.children(node).find(|child| cst.match_rule(*child, rule))
}

/// Full span of a CST node.
pub(crate) fn node_span(cst: &CstData, node: NodeRef) -> Span {
    Span::from(cst.span(node))
}

/// Slices `span` out of the lowered source snapshot.
pub(crate) fn text(source: &str, span: Span) -> &str {
    &source[span.start..span.end]
}
