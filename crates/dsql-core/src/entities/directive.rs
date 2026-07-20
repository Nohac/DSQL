//! Directive entity: `@namespace.member(arg: value)` annotations on
//! definitions, selections, and spreads.

use crate::schema::AstFacts;
use bowl::{Commands, Component, DerivedFrom, Entity, Query, Registrar, SystemExt, View, With};

use crate::entities::expression::{Expr, build_expr, expr_child};
use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{NodeRef, Rule};
use crate::schema::dsql_schema;

/// One directive occurrence, lowered from `directive`. [`ChildOf`] links it
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
    /// Span of the whole `name: value` argument.
    pub span: Span,
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

    /// The resolved name — the shorthand `@.member` reports as
    /// `dsql.member`, matching how the registry resolves it.
    pub fn canonical_name(&self) -> String {
        let namespace = self.namespace.as_deref().unwrap_or(DSQL_NAMESPACE);
        match &self.member {
            Some(member) => format!("{namespace}.{member}"),
            None => namespace.to_string(),
        }
    }
}

/// The namespace the `@.member` shorthand resolves into.
pub const DSQL_NAMESPACE: &str = "dsql";

/// Semantic position a directive annotates, from its parent construct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveLocation {
    Query,
    Field,
    FragmentSpread,
}

impl DirectiveLocation {
    /// The user-facing label directive diagnostics use.
    pub fn label(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Field => "field",
            Self::FragmentSpread => "fragment spread",
        }
    }
}

/// What a directive argument's value must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveArgumentValueKind {
    String,
    BooleanExpression,
}

impl DirectiveArgumentValueKind {
    /// The user-facing type description diagnostics use.
    pub fn label(self) -> &'static str {
        match self {
            Self::String => "a string",
            Self::BooleanExpression => "a boolean expression",
        }
    }

    fn matches(self, value: &Expr) -> bool {
        use crate::entities::expression::LiteralValue;
        match self {
            Self::String => matches!(
                value,
                Expr::Literal {
                    value: LiteralValue::String(_),
                    ..
                }
            ),
            Self::BooleanExpression => matches!(
                value,
                Expr::Literal {
                    value: LiteralValue::Bool(_),
                    ..
                } | Expr::Variable { .. }
                    | Expr::Binary { .. }
            ),
        }
    }
}

/// One argument a system directive declares.
pub struct DirectiveArgumentDefinition {
    pub name: &'static str,
    pub required: bool,
    pub value: DirectiveArgumentValueKind,
}

/// One compiler-owned directive: its name, legal positions, and argument
/// schema.
pub struct SystemDirectiveDefinition {
    pub namespace: &'static str,
    pub member: &'static str,
    pub locations: &'static [DirectiveLocation],
    pub arguments: &'static [DirectiveArgumentDefinition],
    /// Recognized directives whose downstream semantics are not built yet
    /// still error: an annotation that silently changes nothing (when the
    /// spec promises it changes the SQL) is worse than a rejection.
    pub unimplemented_semantics: bool,
}

/// The compiler-owned directives. External definitions were a dead hook
/// in the proof of concept (`register_external` had no callers) and are
/// omitted until something needs them.
pub const SYSTEM_DIRECTIVES: &[SystemDirectiveDefinition] = &[
    SystemDirectiveDefinition {
        namespace: DSQL_NAMESPACE,
        member: "include_if",
        locations: &[DirectiveLocation::Field],
        arguments: &[DirectiveArgumentDefinition {
            name: "if",
            required: true,
            value: DirectiveArgumentValueKind::BooleanExpression,
        }],
        unimplemented_semantics: true,
    },
    SystemDirectiveDefinition {
        namespace: DSQL_NAMESPACE,
        member: "deprecated",
        locations: &[DirectiveLocation::Query, DirectiveLocation::Field],
        arguments: &[DirectiveArgumentDefinition {
            name: "reason",
            required: false,
            value: DirectiveArgumentValueKind::String,
        }],
        unimplemented_semantics: false,
    },
];

/// Resolves a lowered directive name against the system registry. The
/// `@.member` shorthand resolves in the dsql namespace; a bare
/// `@namespace` names no member and resolves nothing.
pub fn resolve_directive(fact: &DirectiveFact) -> Option<&'static SystemDirectiveDefinition> {
    let namespace = fact.namespace.as_deref().unwrap_or(DSQL_NAMESPACE);
    let member = fact.member.as_deref()?;
    SYSTEM_DIRECTIVES
        .iter()
        .find(|definition| definition.namespace == namespace && definition.member == member)
}

impl LanguageEntity for Directive {
    const NAME: &'static str = "directive";

    fn register(reg: &mut Registrar<'_>) {
        reg.system(check_directives.run_during(bowl::Phase::Complete));
        reg.system(complete_directives.run_during(bowl::Phase::Complete));
    }
}

/// Validates one directive occurrence against the registry: unknown
/// names, illegal positions, and the argument schema — the same six
/// diagnostics the proof of concept emitted, with its precedence
/// (unknown name stops; a misplaced directive still checks arguments;
/// a duplicate argument skips its own unknown/type checks).
async fn check_directives(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &DirectiveFact, &ChildOf, &BelongsToFile)>,
    definitions: View<'_, (Entity, &crate::entities::definition::DefDecl)>,
    fields: View<'_, (Entity, &crate::entities::field_selection::FieldSel)>,
    spreads: View<'_, (Entity, &crate::entities::fragment_spread::SpreadDecl)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, directive, parent, file) = query.item();
    let mut report = |span: Span, code: DiagnosticCode, message: String| {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(entity),
                file: file.0,
                span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code,
                message,
            },
        );
    };

    let name = directive.canonical_name();
    let Some(definition) = resolve_directive(directive) else {
        report(
            directive.name_span,
            DiagnosticCode::UnknownDirective,
            format!("unknown directive `@{name}`"),
        );
        return;
    };

    // The parent construct decides the semantic location. The grammar
    // only attaches directives to these three positions.
    let location = if definitions.iter().any(|(def, _)| def == parent.0) {
        Some(DirectiveLocation::Query)
    } else if fields.iter().any(|(field, _)| field == parent.0) {
        Some(DirectiveLocation::Field)
    } else if spreads.iter().any(|(spread, _)| spread == parent.0) {
        Some(DirectiveLocation::FragmentSpread)
    } else {
        None
    };
    if let Some(location) = location
        && !definition.locations.contains(&location)
    {
        report(
            directive.name_span,
            DiagnosticCode::DirectiveNotAllowed,
            format!(
                "directive `@{name}` cannot be used on a {}",
                location.label()
            ),
        );
    }

    let mut seen: Vec<&str> = Vec::new();
    for argument in &directive.arguments {
        if seen.contains(&argument.name.as_str()) {
            report(
                argument.name_span,
                DiagnosticCode::DuplicateDirectiveArgument,
                format!(
                    "directive `@{name}` argument `{}` is given more than once",
                    argument.name
                ),
            );
            continue;
        }
        seen.push(&argument.name);

        let Some(argument_definition) = definition
            .arguments
            .iter()
            .find(|candidate| candidate.name == argument.name)
        else {
            report(
                argument.name_span,
                DiagnosticCode::UnknownDirectiveArgument,
                format!("directive `@{name}` has no argument `{}`", argument.name),
            );
            continue;
        };
        if !argument_definition.value.matches(&argument.value) {
            report(
                argument.span,
                DiagnosticCode::DirectiveArgumentTypeMismatch,
                format!(
                    "directive `@{name}` argument `{}` expects {}",
                    argument.name,
                    argument_definition.value.label()
                ),
            );
        }
    }
    for required in definition
        .arguments
        .iter()
        .filter(|argument| argument.required)
    {
        if !seen.contains(&required.name) {
            report(
                directive.name_span,
                DiagnosticCode::MissingDirectiveArgument,
                format!("directive `@{name}` requires argument `{}`", required.name),
            );
        }
    }

    if definition.unimplemented_semantics {
        report(
            directive.name_span,
            DiagnosticCode::UnsupportedDirective,
            format!("directive `@{name}` is recognized but its semantics are not implemented yet"),
        );
    }
}

/// Contributes registry items for classified directive positions: the
/// namespace (or `.` shorthand) after `@`, a namespace's members, a
/// directive's argument names, and the legal values of boolean arguments.
async fn complete_directives(
    requests: Query<
        (Entity, &crate::service::DirectiveCompletionContext),
        With<crate::service::CompletionRequest>,
    >,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    use crate::service::DirectiveRole;
    use crate::service::completion::{CompletionItem, CompletionKind, emit_completion_candidate};

    let (request, context) = requests.item();
    let directive_item = |label: &str, detail: String, insert: Option<String>| CompletionItem {
        label: label.to_string(),
        kind: CompletionKind::Directive,
        detail: Some(detail),
        documentation: None,
        insert_text: insert,
    };

    let mut items = Vec::new();
    match &context.role {
        DirectiveRole::Name => {
            items.push(directive_item(
                ".",
                format!("the `{DSQL_NAMESPACE}` namespace shorthand"),
                None,
            ));
            let mut namespaces: Vec<&str> = Vec::new();
            for definition in SYSTEM_DIRECTIVES {
                if !namespaces.contains(&definition.namespace) {
                    namespaces.push(definition.namespace);
                    items.push(directive_item(
                        definition.namespace,
                        "directive namespace".to_string(),
                        None,
                    ));
                }
            }
        }
        DirectiveRole::Member { namespace } => {
            for definition in SYSTEM_DIRECTIVES
                .iter()
                .filter(|definition| definition.namespace == namespace)
            {
                let locations = definition
                    .locations
                    .iter()
                    .map(|location| location.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                let note = if definition.unimplemented_semantics {
                    " (semantics not implemented yet)"
                } else {
                    ""
                };
                items.push(directive_item(
                    definition.member,
                    format!("directive on {locations}{note}"),
                    None,
                ));
            }
        }
        DirectiveRole::Argument { namespace, member } => {
            let resolved = SYSTEM_DIRECTIVES.iter().find(|definition| {
                definition.namespace == namespace && definition.member == member
            });
            for argument in resolved
                .map(|definition| definition.arguments)
                .unwrap_or(&[])
            {
                items.push(directive_item(
                    argument.name,
                    format!(
                        "{}{}",
                        argument.value.label(),
                        if argument.required { ", required" } else { "" }
                    ),
                    Some(format!("{}: ", argument.name)),
                ));
            }
        }
        DirectiveRole::Value {
            namespace,
            member,
            argument,
        } => {
            let value = SYSTEM_DIRECTIVES
                .iter()
                .find(|definition| definition.namespace == namespace && definition.member == member)
                .and_then(|definition| {
                    definition
                        .arguments
                        .iter()
                        .find(|candidate| candidate.name == argument)
                })
                .map(|argument| argument.value);
            if value == Some(DirectiveArgumentValueKind::BooleanExpression) {
                // Values are ordinary keywords, not directive names.
                for label in ["true", "false"] {
                    items.push(CompletionItem {
                        label: label.to_string(),
                        kind: CompletionKind::Keyword,
                        detail: None,
                        documentation: None,
                        insert_text: None,
                    });
                }
            }
        }
    }
    emit_completion_candidate(&mut commands, request, items);
}

impl LowerStage for Directive {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let name_node = direct_rule(ctx.cst, node, Rule::DirectiveName)?;

        let namespace = direct_rule(ctx.cst, name_node, Rule::DirectiveNamespace)
            .and_then(|namespace| direct_name(ctx.cst, namespace))
            .map(|span| text(ctx.source, span).to_string());
        let member = direct_rule(ctx.cst, name_node, Rule::DirectiveMember)
            .and_then(|member| direct_name(ctx.cst, member))
            .map(|span| text(ctx.source, span).to_string());
        let shorthand = namespace.is_none() && member.is_some();

        let arguments = ctx
            .cst
            .children(node)
            .filter(|child| ctx.cst.match_rule(*child, Rule::DirectiveArgument))
            .filter_map(|argument| {
                let name_span = direct_name(ctx.cst, argument)?;
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
                    span: node_span(ctx.cst, argument),
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
