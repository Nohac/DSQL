use crate::language::prelude::*;
use facet::Facet;

/// Directive attached to a selection or fragment spread.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct Directive {
    pub range: TextRange,
    pub name: DirectiveName,
    pub arguments: Vec<DirectiveArgument>,
}

/// Parsed directive name split into namespace and optional namespace member.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct DirectiveName {
    pub range: TextRange,
    pub namespace: DirectiveNamespace,
    pub member: Option<NameRef>,
}

/// Namespace form used by a directive name.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum DirectiveNamespace {
    /// Shorthand for the built-in `dsql` namespace, written as `@.<member>`.
    DsqlShorthand { range: TextRange },
    /// Explicit namespace name, such as `dsql` in `@dsql.include_if`.
    Named(NameRef),
}

/// Named directive argument with an expression value.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct DirectiveArgument {
    pub range: TextRange,
    pub name: NameRef,
    pub value: Expr,
}

/// Built-in directive identity known to compiler stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum SystemDirectiveKind {
    IncludeIf,
    Deprecated,
}

/// Semantic location where a directive is attached during checking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum DirectiveLocation {
    Query,
    Field,
    FragmentSpread,
}

impl DirectiveLocation {
    /// Returns the user-facing label used by directive diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Field => "field",
            Self::FragmentSpread => "fragment spread",
        }
    }
}

/// Lightweight built-in directive value category used before JSON Schema support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum DirectiveArgumentValueKind {
    String,
    BooleanExpression,
}

impl DirectiveArgumentValueKind {
    /// Returns the user-facing type description used by diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::String => "a string",
            Self::BooleanExpression => "a boolean expression",
        }
    }
}

/// Static schema entry for a compiler-owned directive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
pub struct SystemDirectiveDefinition {
    pub kind: SystemDirectiveKind,
    pub namespace: &'static str,
    pub member: &'static str,
    /// Semantic locations where the directive may appear.
    pub locations: &'static [DirectiveLocation],
    pub arguments: &'static [DirectiveArgumentDefinition],
}

/// Static directive argument schema used by system directives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
pub struct DirectiveArgumentDefinition {
    pub name: &'static str,
    pub required: bool,
    /// Lightweight expected value category used by the built-in validator.
    pub value: DirectiveArgumentValueKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectiveCompletionContext<'a> {
    Namespace {
        prefix: &'a str,
    },
    Member {
        namespace: &'a str,
        prefix: &'a str,
    },
    Argument {
        directive: &'a str,
        prefix: &'a str,
    },
    Value {
        directive: &'a str,
        argument: &'a str,
        prefix: &'a str,
    },
}

/// Directive definition loaded from external schema metadata.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ExternalDirectiveDefinition {
    pub namespace: String,
    pub member: Option<String>,
}

/// Resolved directive definition, preserving whether the directive is compiler-owned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveDefinition<'a> {
    System(&'static SystemDirectiveDefinition),
    External(&'a ExternalDirectiveDefinition),
}

/// Registry used by directive-aware stages to resolve parsed directive names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Facet)]
pub struct DirectiveRegistry {
    external: Vec<ExternalDirectiveDefinition>,
}

const INCLUDE_IF_ARGUMENTS: &[DirectiveArgumentDefinition] = &[DirectiveArgumentDefinition {
    name: "if",
    required: true,
    value: DirectiveArgumentValueKind::BooleanExpression,
}];

const DEPRECATED_ARGUMENTS: &[DirectiveArgumentDefinition] = &[DirectiveArgumentDefinition {
    name: "reason",
    required: false,
    value: DirectiveArgumentValueKind::String,
}];

const FIELD_LOCATIONS: &[DirectiveLocation] = &[DirectiveLocation::Field];
const QUERY_AND_FIELD_LOCATIONS: &[DirectiveLocation] =
    &[DirectiveLocation::Query, DirectiveLocation::Field];

const SYSTEM_DIRECTIVES: &[SystemDirectiveDefinition] = &[
    SystemDirectiveDefinition {
        kind: SystemDirectiveKind::IncludeIf,
        namespace: "dsql",
        member: "include_if",
        locations: FIELD_LOCATIONS,
        arguments: INCLUDE_IF_ARGUMENTS,
    },
    SystemDirectiveDefinition {
        kind: SystemDirectiveKind::Deprecated,
        namespace: "dsql",
        member: "deprecated",
        locations: QUERY_AND_FIELD_LOCATIONS,
        arguments: DEPRECATED_ARGUMENTS,
    },
];

/// Returns the compiler-owned directive definitions used by checking and editor features.
pub fn system_directive_definitions() -> &'static [SystemDirectiveDefinition] {
    SYSTEM_DIRECTIVES
}

impl DirectiveName {
    /// Returns the directive namespace as compiler-facing text.
    pub fn namespace_text(&self) -> &str {
        match &self.namespace {
            DirectiveNamespace::DsqlShorthand { .. } => "dsql",
            DirectiveNamespace::Named(name) => name.text.as_str(),
        }
    }

    /// Returns the member part after the namespace, when one is present.
    pub fn member_text(&self) -> Option<&str> {
        self.member.as_ref().map(|member| member.text.as_str())
    }

    /// Returns the canonical namespace-qualified directive name.
    pub fn canonical_text(&self) -> String {
        self.member_text().map_or_else(
            || self.namespace_text().to_string(),
            |member| format!("{}.{member}", self.namespace_text()),
        )
    }
}

impl DirectiveRegistry {
    /// Creates a registry containing the compiler-owned directive definitions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an externally supplied directive definition for metadata/codegen use.
    #[expect(
        dead_code,
        reason = "external schema loading will register definitions through this API"
    )]
    pub fn register_external(&mut self, definition: ExternalDirectiveDefinition) {
        self.external.push(definition);
    }

    /// Resolves a parsed directive name against system and external definitions.
    pub fn resolve<'a>(&'a self, name: &DirectiveName) -> Option<DirectiveDefinition<'a>> {
        SYSTEM_DIRECTIVES
            .iter()
            .find(|definition| {
                name.namespace_text() == definition.namespace
                    && name.member_text() == Some(definition.member)
            })
            .map(DirectiveDefinition::System)
            .or_else(|| {
                self.external
                    .iter()
                    .find(|definition| {
                        name.namespace_text() == definition.namespace
                            && name.member_text() == definition.member.as_deref()
                    })
                    .map(DirectiveDefinition::External)
            })
    }
}

/// Context passed from legacy selection traversal into directive checking.
pub struct DirectiveCheckContext<'a, 'errors> {
    /// Registry used to resolve the directive name currently being checked.
    pub registry: &'a DirectiveRegistry,
    /// Semantic location of the directive's syntax owner.
    pub location: DirectiveLocation,
    /// Check diagnostics produced by directive validation.
    pub errors: &'errors mut Vec<CheckError>,
}

/// Language atom that owns directive parsing, formatting, lowering, and checks.
pub enum DirectiveAtom {}

/// Lowered directive identity captured during context-free lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredDirective {
    pub name: NameId,
}

language_atom! {
    DirectiveAtom {
        grammar_rule: Rule::Directive,
        ast: Directive,
        lowered: LoweredDirective,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("directives do not produce lint diagnostics until directive semantics exist"),
        variables: required,
        plan: required,
        sql: no_effect("directives affect checked semantics and plans before SQL generation"),
        metadata: required,
        editor: required,
    }
}

impl BuildsAst<DirectiveAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> Directive {
        let name = self
            .direct_rule(node, Rule::DirectiveName)
            .map(|name| self.directive_name(name))
            .unwrap_or_else(|| DirectiveName {
                range: self.node_range(node),
                namespace: DirectiveNamespace::Named(self.missing_name(node)),
                member: None,
            });
        let arguments = self
            .direct_rules(node, Rule::DirectiveArgument)
            .into_iter()
            .map(|argument| self.directive_argument(argument))
            .collect();
        Directive {
            range: self.node_range(node),
            name,
            arguments,
        }
    }
}

impl AstBuilder<'_> {
    fn directive_name(&self, node: NodeRef) -> DirectiveName {
        let names = self.direct_names(node);
        let range = self.node_range(node);
        if let Some((Token::Dot, dot_range)) = self.first_direct_token(node) {
            return DirectiveName {
                range,
                namespace: DirectiveNamespace::DsqlShorthand { range: dot_range },
                member: names
                    .first()
                    .cloned()
                    .or_else(|| Some(self.missing_name(node))),
            };
        }

        let namespace = names
            .first()
            .cloned()
            .unwrap_or_else(|| self.missing_name(node));
        DirectiveName {
            range,
            namespace: DirectiveNamespace::Named(namespace),
            member: names.get(1).cloned(),
        }
    }

    fn directive_argument(&self, node: NodeRef) -> DirectiveArgument {
        DirectiveArgument {
            range: self.node_range(node),
            name: self
                .direct_names(node)
                .into_iter()
                .next()
                .unwrap_or_else(|| self.missing_name(node)),
            value: self
                .direct_value_rule(node)
                .map(|expr| self.expr(expr))
                .unwrap_or_else(|| Expr::Name(self.missing_name(node))),
        }
    }
}

impl Formats<DirectiveAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(" ");
        self.write_str(&text);
    }
}

impl Lowers<DirectiveAtom> for Lowerer {
    fn lower(
        directive: &Directive,
        interner: &mut Interner,
        names: &mut NameIndex,
    ) -> LoweredDirective {
        let name = interner.intern(&directive.name.canonical_text());
        names.directives.push((name, directive.name.range));
        for argument in &directive.arguments {
            interner.intern(&argument.name.text);
        }
        LoweredDirective { name }
    }
}

impl Checks<DirectiveAtom> for Checker {
    type Context<'a> = DirectiveCheckContext<'a, 'a>;

    fn check(directive: &Directive, context: Self::Context<'_>) {
        match context.registry.resolve(&directive.name) {
            Some(DirectiveDefinition::System(definition)) => {
                check_system_directive(directive, definition, context.location, context.errors);
            }
            Some(DirectiveDefinition::External(_)) => {}
            None => context.errors.push(CheckError {
                range: directive.name.range,
                kind: CheckErrorKind::UnknownDirective {
                    name: directive.name.canonical_text(),
                },
            }),
        }
    }
}

impl DirectiveAtom {
    /// Checks all directives attached to one syntax owner at the given semantic location.
    pub fn check_all(
        directives: &[Directive],
        registry: &DirectiveRegistry,
        location: DirectiveLocation,
        errors: &mut Vec<CheckError>,
    ) {
        for directive in directives {
            <Checker as Checks<DirectiveAtom>>::check(
                directive,
                DirectiveCheckContext {
                    registry,
                    location,
                    errors,
                },
            );
        }
    }
}

impl Completer<DirectiveAtom> for LanguageService {
    fn completions(request: EditorCompletionRequest<'_>) -> Vec<EditorCompletion> {
        let before = request.source.text(TextRange::new(0, request.byte));
        let Some(directive_start) = before.as_ref().rfind('@') else {
            return Vec::new();
        };
        if before.as_ref()[directive_start..]
            .chars()
            .any(|character| matches!(character, '{' | '}' | '\n'))
        {
            return Vec::new();
        }

        let directive = &before.as_ref()[directive_start + 1..];
        let Some(context) = directive_completion_context(directive) else {
            return Vec::new();
        };

        match context {
            DirectiveCompletionContext::Namespace { prefix } => namespace_completions(prefix),
            DirectiveCompletionContext::Member { namespace, prefix } => {
                member_completions(namespace, prefix)
            }
            DirectiveCompletionContext::Argument { directive, prefix } => {
                argument_completions(directive, prefix)
            }
            DirectiveCompletionContext::Value {
                directive,
                argument,
                prefix,
            } => value_completions(directive, argument, prefix),
        }
    }
}

fn directive_completion_context(directive: &str) -> Option<DirectiveCompletionContext<'_>> {
    if let Some(open_paren) = directive.rfind('(')
        && !directive[open_paren + 1..].contains(')')
    {
        let directive_name = &directive[..open_paren];
        let current_argument = directive[open_paren + 1..]
            .rsplit_once(',')
            .map_or(&directive[open_paren + 1..], |(_, argument)| argument)
            .trim_start();
        return if let Some((argument, prefix)) = current_argument.split_once(':') {
            Some(DirectiveCompletionContext::Value {
                directive: directive_name,
                argument: argument.trim(),
                prefix: prefix.trim_start(),
            })
        } else {
            Some(DirectiveCompletionContext::Argument {
                directive: directive_name,
                prefix: current_argument.trim(),
            })
        };
    }

    if directive.is_empty() || is_name_prefix(directive) {
        return Some(DirectiveCompletionContext::Namespace { prefix: directive });
    }
    if let Some(prefix) = directive.strip_prefix('.') {
        return Some(DirectiveCompletionContext::Member {
            namespace: "dsql",
            prefix,
        });
    }
    if let Some(prefix) = directive.strip_prefix("dsql.") {
        return Some(DirectiveCompletionContext::Member {
            namespace: "dsql",
            prefix,
        });
    }
    None
}

fn namespace_completions(prefix: &str) -> Vec<EditorCompletion> {
    let mut completions = Vec::new();
    if ".".starts_with(prefix) {
        completions.push(EditorCompletion {
            label: ".".to_string(),
            kind: EditorCompletionKind::Directive,
            detail: Some("dsql directive shorthand".to_string()),
            insert_text: None,
        });
    }
    if "dsql".starts_with(prefix) {
        completions.push(EditorCompletion {
            label: "dsql".to_string(),
            kind: EditorCompletionKind::Directive,
            detail: Some("directive namespace".to_string()),
            insert_text: None,
        });
    }
    completions
}

fn member_completions(namespace: &str, prefix: &str) -> Vec<EditorCompletion> {
    system_directive_definitions()
        .iter()
        .filter(|definition| {
            definition.namespace == namespace && definition.member.starts_with(prefix)
        })
        .map(|definition| EditorCompletion {
            label: definition.member.to_string(),
            kind: EditorCompletionKind::Directive,
            detail: Some(format!("dsql directive ({:?})", definition.kind)),
            insert_text: None,
        })
        .collect()
}

fn argument_completions(directive: &str, prefix: &str) -> Vec<EditorCompletion> {
    let Some(definition) = directive_definition_for_syntax_name(directive) else {
        return Vec::new();
    };
    definition
        .arguments
        .iter()
        .filter(|argument| argument.name.starts_with(prefix))
        .map(|argument| EditorCompletion {
            label: argument.name.to_string(),
            kind: EditorCompletionKind::Directive,
            detail: Some("directive argument".to_string()),
            insert_text: Some(format!("{}: ", argument.name)),
        })
        .collect()
}

fn value_completions(directive: &str, argument: &str, prefix: &str) -> Vec<EditorCompletion> {
    let Some(definition) = directive_definition_for_syntax_name(directive) else {
        return Vec::new();
    };
    let Some(argument) = definition
        .arguments
        .iter()
        .find(|definition| definition.name == argument)
    else {
        return Vec::new();
    };

    match argument.value {
        DirectiveArgumentValueKind::BooleanExpression => ["true", "false"]
            .into_iter()
            .filter(|label| label.starts_with(prefix))
            .map(|label| EditorCompletion {
                label: label.to_string(),
                kind: EditorCompletionKind::Keyword,
                detail: Some("boolean".to_string()),
                insert_text: None,
            })
            .collect(),
        DirectiveArgumentValueKind::String => Vec::new(),
    }
}

fn is_name_prefix(value: &str) -> bool {
    value
        .chars()
        .all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn directive_definition_for_syntax_name(name: &str) -> Option<&'static SystemDirectiveDefinition> {
    let member = name
        .strip_prefix('.')
        .or_else(|| name.strip_prefix("dsql."))?;
    system_directive_definitions()
        .iter()
        .find(|definition| definition.namespace == "dsql" && definition.member == member)
}

fn check_system_directive(
    directive: &Directive,
    definition: &SystemDirectiveDefinition,
    location: DirectiveLocation,
    errors: &mut Vec<CheckError>,
) {
    let directive_name = directive.name.canonical_text();
    if !definition.locations.contains(&location) {
        errors.push(CheckError {
            range: directive.name.range,
            kind: CheckErrorKind::DirectiveNotAllowed {
                name: directive_name.clone(),
                location: location.label().to_string(),
            },
        });
    }

    let mut seen_arguments = Vec::new();
    for argument in &directive.arguments {
        if seen_arguments
            .iter()
            .any(|seen: &String| seen == &argument.name.text)
        {
            errors.push(CheckError {
                range: argument.name.range,
                kind: CheckErrorKind::DuplicateDirectiveArgument {
                    name: directive_name.clone(),
                    argument: argument.name.text.clone(),
                },
            });
            continue;
        }
        seen_arguments.push(argument.name.text.clone());

        let Some(argument_definition) = definition
            .arguments
            .iter()
            .find(|definition| definition.name == argument.name.text)
        else {
            errors.push(CheckError {
                range: argument.name.range,
                kind: CheckErrorKind::UnknownDirectiveArgument {
                    name: directive_name.clone(),
                    argument: argument.name.text.clone(),
                },
            });
            continue;
        };

        if !directive_argument_matches(&argument.value, argument_definition.value) {
            errors.push(CheckError {
                range: argument.range,
                kind: CheckErrorKind::DirectiveArgumentTypeMismatch {
                    name: directive_name.clone(),
                    argument: argument.name.text.clone(),
                    expected: argument_definition.value.label().to_string(),
                },
            });
        }
    }

    for required_argument in definition
        .arguments
        .iter()
        .filter(|argument| argument.required)
    {
        if !seen_arguments
            .iter()
            .any(|argument| argument == required_argument.name)
        {
            errors.push(CheckError {
                range: directive.name.range,
                kind: CheckErrorKind::MissingDirectiveArgument {
                    name: directive_name.clone(),
                    argument: required_argument.name.to_string(),
                },
            });
        }
    }
}

fn directive_argument_matches(value: &Expr, kind: DirectiveArgumentValueKind) -> bool {
    match kind {
        DirectiveArgumentValueKind::String => {
            matches!(value, Expr::Literal(Literal::String { .. }))
        }
        DirectiveArgumentValueKind::BooleanExpression => matches!(
            value,
            Expr::Literal(Literal::Bool { .. }) | Expr::Variable(_) | Expr::Binary { .. }
        ),
    }
}

impl InfersVariables<DirectiveAtom> for VariableInference {}

impl Plans<DirectiveAtom> for Planner {}

impl GeneratesMetadata<DirectiveAtom> for MetadataGenerator {}
