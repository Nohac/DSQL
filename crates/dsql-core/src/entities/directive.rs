//! Directive entity: `@namespace.member(arg: value)` annotations on
//! definitions, selections, and spreads.

use bowl::{Bowl, Commands, Component, DerivedFrom};

use crate::entities::expression::{Expr, build_expr, expr_child};
use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{BelongsToFile, NodeKey, ParentKey, Span};
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};

/// One directive occurrence, lowered from `directive`. `ParentKey` links it
/// to the definition, selection, or spread it annotates.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct DirectiveFact {
    /// Explicit namespace, e.g. `dsql` in `@dsql.include_if`. `None` for
    /// the `@.member` shorthand (which resolves to the dsql namespace) and
    /// for bare `@namespace` invocations.
    pub namespace: Option<String>,
    /// `true` when written with the `@.member` dsql shorthand.
    pub shorthand: bool,
    /// The member name, e.g. `include_if`. `None` for bare `@namespace`.
    pub member: Option<String>,
    pub arguments: Vec<DirectiveArgument>,
    /// Span of the full directive name after `@`.
    pub name_span: Span,
    pub span: Span,
}

/// One `name: value` argument of a directive invocation.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct DirectiveArgument {
    pub name: String,
    pub name_span: Span,
    pub value: Expr,
}

/// Owns `directive` (and consumes `directive_name`, `directive_namespace`,
/// `directive_member`, and `directive_argument` from it).
pub struct Directive;

impl LanguageEntity for Directive {
    const NAME: &'static str = "directive";

    async fn register(_bowl: &Bowl) {
        // Directive registry checks land in phase 6.
    }
}

impl LowerStage for Directive {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
        let Some(name_node) = direct_rule(ctx.cst, node, Rule::DirectiveName) else {
            return;
        };

        let namespace = direct_rule(ctx.cst, name_node, Rule::DirectiveNamespace)
            .and_then(|namespace| direct_token(ctx.cst, namespace, Token::Name))
            .map(|span| text(ctx.source, span).to_string());
        let member = direct_rule(ctx.cst, name_node, Rule::DirectiveMember)
            .and_then(|member| direct_token(ctx.cst, member, Token::Name))
            .map(|span| text(ctx.source, span).to_string());
        let shorthand = namespace.is_none() && member.is_some();

        let arguments = ctx
            .cst
            .children(node)
            .filter(|child| ctx.cst.match_rule(*child, Rule::DirectiveArgument))
            .filter_map(|argument| {
                let name_span = direct_token(ctx.cst, argument, Token::Name)?;
                let value = match expr_child(ctx.cst, argument) {
                    Some(expr) => build_expr(ctx.cst, ctx.source, expr),
                    None => Expr::Error {
                        span: node_span(ctx.cst, argument),
                    },
                };
                Some(DirectiveArgument {
                    name: text(ctx.source, name_span).to_string(),
                    name_span,
                    value,
                })
            })
            .collect();

        let fact = DirectiveFact {
            namespace,
            shorthand,
            member,
            arguments,
            name_span: node_span(ctx.cst, name_node),
            span: node_span(ctx.cst, node),
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
                fact,
            )),
            None => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                fact,
            )),
        };
    }
}
