//! The shared lowering walk and rule-ownership dispatch.
//!
//! [`lower_syntax_facts`] is the one generic CST walk: it visits every rule node
//! once and hands it to the entity that owns the rule. Ownership lives in
//! `lower_rule` — an exhaustive `match`, so adding a rule to `dsql.llw`
//! fails to compile here until an entity claims it or it is explicitly
//! listed as structural.

use crate::schema::AstFacts;
use bowl::{Commands, Entity, Query};

use super::aggregate::Aggregate;
use super::clause::Clause;
use super::definition::Definition;
use super::directive::Directive;
use super::document::{Document, ParsedFile};
use super::expression::Expression;
use super::field_selection::FieldSelection;
use super::fragment_spread::FragmentSpread;
use super::policy::Policy;
use super::variable::Variable;
use crate::entity::{LowerCtx, LowerStage};
use crate::facts::Span;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{CstData, Node, NodeRef, Rule};
use crate::source::ResolutionScope;

fn lower_rule(
    ctx: &LowerCtx<'_>,
    rule: Rule,
    node: NodeRef,
    commands: &mut Commands<AstFacts>,
) -> Option<Entity> {
    match rule {
        Rule::Document => Document::lower(ctx, node, commands),
        Rule::QueryDef | Rule::FragmentDef => Definition::lower(ctx, node, commands),
        Rule::FilterDef | Rule::ConditionDef => Policy::lower(ctx, node, commands),
        Rule::FieldSelection => FieldSelection::lower(ctx, node, commands),
        Rule::PipeTransform => Aggregate::lower(ctx, node, commands),
        Rule::FragmentSpread => FragmentSpread::lower(ctx, node, commands),
        Rule::FilterAssignment
        | Rule::WhereClause
        | Rule::OrderByClause
        | Rule::LimitClause
        | Rule::OffsetClause => Clause::lower(ctx, node, commands),
        Rule::Directive => Directive::lower(ctx, node, commands),
        Rule::ValueVariable | Rule::OperatorVariable => Variable::lower(ctx, node, commands),
        // Expression rules lower as part of the clause/directive facts that
        // contain them (expression::build_expr); the claim is a no-op.
        Rule::Expr
        | Rule::BinaryExpr
        | Rule::UnaryExpr
        | Rule::NullTestExpr
        | Rule::ScalarAggregateExpr
        | Rule::CollectionLiteral
        | Rule::ExistsExpr
        | Rule::ExistsSource
        | Rule::PredicateName
        | Rule::Literal
        | Rule::BinaryOperator
        | Rule::ComparisonOperator
        | Rule::ScopedPath
        | Rule::ScopedPathSegment => Expression::lower(ctx, node, commands),

        // Consumed by FieldSelection lowering from the field_selection node.
        Rule::ContextualName | Rule::FieldSelectionTail | Rule::FieldSuffix => None,
        // Consumed by Policy lowering from the definition root.
        Rule::PolicyTarget
        | Rule::ShapeTarget
        | Rule::ShapeField
        | Rule::FilterBody
        | Rule::FilterRule
        | Rule::ConditionBody
        | Rule::ApplyRule
        | Rule::FieldRule => None,
        // Consumed by Aggregate lowering from the pipe_transform node.
        Rule::AggregateField | Rule::AggregateGroupKey | Rule::AggregateSet => None,
        // Consumed by Clause lowering from the clause nodes.
        Rule::Clause
        | Rule::ClauseList
        | Rule::QueryFilterHeader
        | Rule::OrderItem
        | Rule::SortDirection => None,
        // Consumed by Directive lowering from the directive node.
        Rule::DirectiveName
        | Rule::DirectiveNamespace
        | Rule::DirectiveMember
        | Rule::DirectiveArgument => None,
        // Consumed by Definition (fragment targets), FieldSelection
        // (relation refs), and Clause (order items) lowerings.
        Rule::QualifiedName | Rule::RelationRef => None,

        // Structural rules: consumed by the entities owning their
        // ancestors, never owned themselves.
        Rule::Definition | Rule::Selection | Rule::SelectionSet => None,

        // Error recovery nodes surface through parse diagnostics, not
        // through lowering.
        Rule::Error => None,
    }
}

/// The one generic lowering walk: one invocation per parsed file, visiting
/// every CST rule node once and dispatching to the owning entity.
pub async fn lower_syntax_facts(
    query: Query<(Entity, &ParsedFile, &ResolutionScope)>,
    mut commands: Commands<AstFacts>,
) {
    let (file, parsed, scope) = query.item();

    let ctx = LowerCtx {
        cst: &parsed.cst,
        source: &parsed.source,
        file,
        scope: &scope.0,
        parent: None,
    };
    walk(&ctx, NodeRef::ROOT, &mut commands);
}

fn walk(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands<AstFacts>) {
    if let Node::Rule(rule, _) = ctx.cst.get(node) {
        let created = lower_rule(ctx, rule, node, commands);

        // Definitions and field selections form the selection tree; spreads,
        // clauses, and directives are attachment points for the facts nested
        // inside them (directives on spreads, variables in clauses). Descend
        // with this node's fact entity as the parent so nested facts carry
        // their position as a `ChildOf` edge. A tree rule that lowered
        // nothing (error recovery) orphans its descendants, matching the
        // old dangling-key behavior.
        if matches!(
            rule,
            Rule::QueryDef
                | Rule::FragmentDef
                | Rule::FilterDef
                | Rule::ConditionDef
                | Rule::FieldSelection
                | Rule::PipeTransform
                | Rule::FragmentSpread
                | Rule::WhereClause
                | Rule::OrderByClause
                | Rule::LimitClause
                | Rule::OffsetClause
                | Rule::FilterAssignment
                | Rule::Directive
        ) {
            let scoped = LowerCtx {
                cst: ctx.cst,
                source: ctx.source,
                file: ctx.file,
                scope: ctx.scope,
                parent: created,
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

/// Spans of direct children accepted as source identifiers. Ordinary names
/// remain tokens; predicate keywords use [`Rule::ContextualName`].
pub(crate) fn direct_names(cst: &CstData, node: NodeRef) -> Vec<Span> {
    cst.children(node)
        .filter_map(|child| match cst.get(child) {
            crate::grammar::parser::Node::Token(Token::Name, _) => Some(node_span(cst, child)),
            crate::grammar::parser::Node::Rule(Rule::ContextualName, _) => {
                Some(node_span(cst, child))
            }
            _ => None,
        })
        .collect()
}

/// Span of the first direct source identifier under `node`.
pub(crate) fn direct_name(cst: &CstData, node: NodeRef) -> Option<Span> {
    direct_names(cst, node).into_iter().next()
}

/// First direct child of `node` that is a `rule` node.
pub(crate) fn direct_rule(cst: &CstData, node: NodeRef, rule: Rule) -> Option<NodeRef> {
    cst.children(node)
        .find(|child| cst.match_rule(*child, rule))
}

/// Full span of a CST node.
pub(crate) fn node_span(cst: &CstData, node: NodeRef) -> Span {
    Span::from(cst.span(node))
}

/// Slices `span` out of the lowered source snapshot.
pub(crate) fn text(source: &str, span: Span) -> &str {
    &source[span.start..span.end]
}

/// Rule-ownership dispatch for the format stage, mirroring [`lower_rule`]:
/// exhaustive over the rules the formatter hands to entities. Structural
/// rules are laid out by the engine itself (selection sets, clause lists)
/// or consumed by their owner's formatting.
pub fn format_rule(
    formatter: &mut crate::format::CstFormatter<'_>,
    rule: Rule,
    node: NodeRef,
) -> bool {
    use crate::entity::FormatStage;

    match rule {
        Rule::Document => Document::format(formatter, node),
        Rule::QueryDef | Rule::FragmentDef => Definition::format(formatter, node),
        Rule::FilterDef | Rule::ConditionDef => Policy::format(formatter, node),
        Rule::FieldSelection => FieldSelection::format(formatter, node),
        Rule::PipeTransform => Aggregate::format(formatter, node),
        Rule::FragmentSpread => FragmentSpread::format(formatter, node),
        Rule::FilterAssignment
        | Rule::WhereClause
        | Rule::OrderByClause
        | Rule::LimitClause
        | Rule::OffsetClause => Clause::format(formatter, node),
        Rule::Directive => Directive::format(formatter, node),
        Rule::ValueVariable | Rule::OperatorVariable => Variable::format(formatter, node),
        Rule::Expr
        | Rule::BinaryExpr
        | Rule::UnaryExpr
        | Rule::NullTestExpr
        | Rule::ScalarAggregateExpr
        | Rule::CollectionLiteral
        | Rule::ExistsExpr
        | Rule::ExistsSource
        | Rule::PredicateName
        | Rule::Literal
        | Rule::BinaryOperator
        | Rule::ComparisonOperator
        | Rule::ScopedPath
        | Rule::ScopedPathSegment => Expression::format(formatter, node),
        // Structural rules: laid out by the engine or their owner.
        Rule::FieldSelectionTail
        | Rule::ContextualName
        | Rule::FieldSuffix
        | Rule::AggregateField
        | Rule::AggregateGroupKey
        | Rule::AggregateSet
        | Rule::Clause
        | Rule::ClauseList
        | Rule::OrderItem
        | Rule::SortDirection
        | Rule::QueryFilterHeader
        | Rule::PolicyTarget
        | Rule::ShapeTarget
        | Rule::ShapeField
        | Rule::FilterBody
        | Rule::FilterRule
        | Rule::ConditionBody
        | Rule::ApplyRule
        | Rule::FieldRule
        | Rule::DirectiveName
        | Rule::DirectiveNamespace
        | Rule::DirectiveMember
        | Rule::DirectiveArgument
        | Rule::QualifiedName
        | Rule::RelationRef
        | Rule::Definition
        | Rule::Selection
        | Rule::SelectionSet
        | Rule::Error => return false,
    }
    true
}
