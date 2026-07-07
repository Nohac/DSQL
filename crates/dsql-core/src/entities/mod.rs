//! Language entities and the rule-ownership dispatch.
//!
//! [`generate_ast`] is the one generic CST walk: it visits every rule node
//! once and hands it to the entity that owns the rule. Ownership lives in
//! [`lower_rule`] — an exhaustive `match`, so adding a rule to `dsql.llw`
//! fails to compile here until an entity claims it or it is explicitly
//! listed as structural.

pub mod document;

use bowl::{Commands, Entity, Query};

use crate::entity::{LowerCtx, LowerStage};
use crate::grammar::parser::{Node, NodeRef, Rule};
use document::{Document, ParsedFile};

fn lower_rule(ctx: &LowerCtx<'_>, rule: Rule, node: NodeRef, commands: &mut Commands) {
    match rule {
        Rule::Document => Document::lower(ctx, node, commands),

        // Unclaimed rules, owned by entities scheduled in docs/plan.md.
        // Move a rule up as its entity lands; do not lower it here.
        Rule::QueryDef | Rule::FragmentDef => {} // phase 3: QueryDef, FragmentDef
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
