//! Language entities and the rule-ownership dispatch.
//!
//! [`generate_ast`] is the one generic CST walk: it visits every rule node
//! once and hands it to the entity that owns the rule. Ownership lives in
//! [`lower_rule`] — an exhaustive `match`, so adding a rule to `dsql.llw`
//! fails to compile here until an entity claims it or it is explicitly
//! listed as structural.

pub mod definition;
pub mod document;

use bowl::{Commands, Entity, Query};

use crate::entity::{LowerCtx, LowerStage};
use crate::facts::Span;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{CstData, Node, NodeRef, Rule};
use definition::Definition;
use document::{Document, ParsedFile};

fn lower_rule(ctx: &LowerCtx<'_>, rule: Rule, node: NodeRef, commands: &mut Commands) {
    match rule {
        Rule::Document => Document::lower(ctx, node, commands),
        Rule::QueryDef | Rule::FragmentDef => Definition::lower(ctx, node, commands),

        // Unclaimed rules, owned by entities scheduled in docs/plan.md.
        // Move a rule up as its entity lands; do not lower it here.
        Rule::FieldSelection | Rule::FieldSelectionTail | Rule::FieldSuffix => {} // phase 4
        Rule::FragmentSpread => {} // phase 4
        Rule::Clause
        | Rule::ClauseList
        | Rule::WhereClause
        | Rule::OrderByClause
        | Rule::OrderItem
        | Rule::SortDirection
        | Rule::LimitClause
        | Rule::OffsetClause => {} // phase 5: Clause
        Rule::Directive
        | Rule::DirectiveName
        | Rule::DirectiveNamespace
        | Rule::DirectiveMember
        | Rule::DirectiveArgument => {} // phase 5: Directive
        Rule::Expr | Rule::BinaryExpr | Rule::Literal => {} // phase 5: Expression
        Rule::BinaryOperator | Rule::ComparisonOperator => {} // phase 5: Expression
        Rule::ScopedPath | Rule::ScopedPathSegment | Rule::QualifiedName | Rule::RelationRef => {} // phase 5: Path
        Rule::ValueVariable | Rule::OperatorVariable => {} // phase 5: Variable

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
    };
    walk(&ctx, NodeRef::ROOT, &mut commands);
}

fn walk(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
    if let Node::Rule(rule, _) = ctx.cst.get(node) {
        lower_rule(ctx, rule, node, commands);
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
