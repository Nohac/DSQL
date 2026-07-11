//! Directive entity: `@namespace.member(arg: value)` annotations on
//! definitions, selections, and spreads.

use crate::schema::AstFacts;
use bowl::{Commands, Component, DerivedFrom, Entity, Query, Registrar, With};

use crate::entities::expression::{Expr, build_expr, expr_child};
use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{
    CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage,
};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::schema::dsql_schema;

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

impl DirectiveFact {
    /// The written name after `@`, for diagnostics and services.
    pub fn display_name(&self) -> String {
        match (&self.namespace, &self.member) {
            (Some(namespace), Some(member)) => format!("{namespace}.{member}"),
            (Some(namespace), None) => namespace.clone(),
            (None, Some(member)) => format!(".{member}"),
            (None, None) => String::new(),
        }
    }
}

impl LanguageEntity for Directive {
    const NAME: &'static str = "directive";

    fn register(reg: &mut Registrar<'_>) {
        // Until the checked directive registry is ported, every directive
        // is rejected: parsing-but-ignoring would silently drop semantics
        // the directive spec promises (docs/spec/directives.md).
        reg.system(check_directives_unsupported);
    }
}

/// Every directive is an error until directive semantics are ported —
/// accepted-but-ignored annotations are worse than rejected ones.
async fn check_directives_unsupported(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &DirectiveFact, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, directive, file) = query.item();
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(entity),
            file: file.0,
            span: directive.name_span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code: DiagnosticCode::UnsupportedDirective,
            message: format!(
                "directive `@{}` is not supported yet",
                directive.display_name()
            ),
        },
    );
}

impl LowerStage for Directive {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let name_node = direct_rule(ctx.cst, node, Rule::DirectiveName)?;

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

        let entity = match ctx.parent {
            Some(parent) => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    fact,
                    ChildOf(parent),
                ))
                .untyped(),
            None => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    fact,
                ))
                .untyped(),
        };
        Some(entity)
    }
}

impl FormatStage for Directive {
    /// Directives are preserved verbatim, preceded by one space.
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.write_str(" ");
        formatter.write_node_text(node);
    }
}

impl HoverStage for Directive {
    /// Directive hover lands with the directive registry.
    fn register_hover(_reg: &mut Registrar<'_>) {}
}

impl CompletionStage for Directive {
    /// Directive completions land with the directive registry.
    fn register_completions(_reg: &mut Registrar<'_>) {}
}
