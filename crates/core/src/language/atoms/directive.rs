use crate::language::prelude::*;
use facet::Facet;
use std::borrow::Cow;

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

/// Schema entry for a compiler-owned directive stored in a directive registry.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct SystemDirectiveDefinition {
    pub kind: SystemDirectiveKind,
    pub namespace: String,
    pub member: String,
    /// Semantic locations where the directive may appear.
    pub locations: Vec<DirectiveLocation>,
    pub arguments: Vec<DirectiveArgumentDefinition>,
}

/// Directive argument schema used by system directives.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct DirectiveArgumentDefinition {
    pub name: String,
    pub required: bool,
    /// Lightweight expected value category used by the built-in validator.
    pub value: DirectiveArgumentValueKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Generic context ranges produced by directive context classification.
///
/// The type is directive-local, but it only carries generic syntax data:
/// a [`SyntaxRule`], optional CST node, and the construct/focus ranges that a
/// language-service consumer can read. It intentionally does not encode a
/// directive-specific completion enum.
struct DirectiveSourceWindow {
    construct_range: TextRange,
    focus_range: TextRange,
    rule: SyntaxRule,
    node: Option<usize>,
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
    System(&'a SystemDirectiveDefinition),
    External(&'a ExternalDirectiveDefinition),
}

/// Registry used by directive-aware stages to resolve parsed directive names.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct DirectiveRegistry {
    system: Vec<SystemDirectiveDefinition>,
    external: Vec<ExternalDirectiveDefinition>,
}

impl Default for DirectiveRegistry {
    fn default() -> Self {
        Self::system()
    }
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
        Self::system()
    }

    /// Creates a registry containing the compiler-owned directive definitions.
    pub fn system() -> Self {
        Self {
            system: vec![
                SystemDirectiveDefinition {
                    kind: SystemDirectiveKind::IncludeIf,
                    namespace: "dsql".to_string(),
                    member: "include_if".to_string(),
                    locations: vec![DirectiveLocation::Field],
                    arguments: vec![DirectiveArgumentDefinition {
                        name: "if".to_string(),
                        required: true,
                        value: DirectiveArgumentValueKind::BooleanExpression,
                    }],
                },
                SystemDirectiveDefinition {
                    kind: SystemDirectiveKind::Deprecated,
                    namespace: "dsql".to_string(),
                    member: "deprecated".to_string(),
                    locations: vec![DirectiveLocation::Query, DirectiveLocation::Field],
                    arguments: vec![DirectiveArgumentDefinition {
                        name: "reason".to_string(),
                        required: false,
                        value: DirectiveArgumentValueKind::String,
                    }],
                },
            ],
            external: Vec::new(),
        }
    }

    /// Registers an externally supplied directive definition for metadata/codegen use.
    pub fn register_external(&mut self, definition: ExternalDirectiveDefinition) {
        self.external.push(definition);
    }

    /// Resolves a parsed directive name against system and external definitions.
    pub fn resolve<'a>(&'a self, name: &DirectiveName) -> Option<DirectiveDefinition<'a>> {
        self.system
            .iter()
            .find(|definition| {
                name.namespace_text() == definition.namespace
                    && name.member_text() == Some(definition.member.as_str())
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

    /// Resolves a directive name spelled in source-window completion syntax.
    pub(crate) fn resolve_syntax_name<'a>(&'a self, name: &str) -> Option<DirectiveDefinition<'a>> {
        let (namespace, member) = name
            .strip_prefix('.')
            .map_or_else(|| name.split_once('.'), |member| Some(("dsql", member)))?;
        self.system
            .iter()
            .find(|definition| definition.namespace == namespace && definition.member == member)
            .map(DirectiveDefinition::System)
            .or_else(|| {
                self.external
                    .iter()
                    .find(|definition| {
                        definition.namespace == namespace
                            && definition.member.as_deref() == Some(member)
                    })
                    .map(DirectiveDefinition::External)
            })
    }

    /// Returns directive namespaces known to this registry.
    pub(crate) fn namespace_names(&self) -> Vec<&str> {
        let mut namespaces = Vec::new();
        for definition in &self.system {
            if !namespaces.contains(&definition.namespace.as_str()) {
                namespaces.push(definition.namespace.as_str());
            }
        }
        for definition in &self.external {
            if !namespaces.contains(&definition.namespace.as_str()) {
                namespaces.push(definition.namespace.as_str());
            }
        }
        namespaces
    }

    /// Returns system directive definitions for a namespace.
    pub(crate) fn system_members(&self, namespace: &str) -> Vec<&SystemDirectiveDefinition> {
        self.system
            .iter()
            .filter(|definition| definition.namespace == namespace)
            .collect()
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
        let range = self.node_range(node);
        if let Some((Token::Dot, dot_range)) = self.first_direct_token(node) {
            return DirectiveName {
                range,
                namespace: DirectiveNamespace::DsqlShorthand { range: dot_range },
                member: self
                    .directive_member(node)
                    .or_else(|| Some(self.missing_name(node))),
            };
        }

        let namespace = self
            .directive_namespace(node)
            .unwrap_or_else(|| self.missing_name(node));
        DirectiveName {
            range,
            namespace: DirectiveNamespace::Named(namespace),
            member: self.directive_member(node),
        }
    }

    fn directive_namespace(&self, node: NodeRef) -> Option<NameRef> {
        self.direct_rule(node, Rule::DirectiveNamespace)
            .and_then(|namespace| self.direct_names(namespace).into_iter().next())
    }

    fn directive_member(&self, node: NodeRef) -> Option<NameRef> {
        self.direct_rule(node, Rule::DirectiveMember)
            .and_then(|member| self.direct_names(member).into_iter().next())
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

impl ProvidesContext<DirectiveAtom> for LanguageService {
    /// Refines cursor evidence into precise directive syntax contexts.
    ///
    /// The order is part of the contract for future atoms:
    /// 1. use parsed CST structure when available;
    /// 2. use parser recovery/expected-token evidence for malformed positions;
    /// 3. use bounded source-window recovery only as a fallback.
    fn contexts<'a>(input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        if let Some(window) = directive_cst_window(input) {
            return vec![directive_context(
                input,
                window,
                ContextOrigin::Cst,
                ContextConfidence::Exact,
            )];
        }

        if let Some(window) = directive_expected_token_window(input) {
            return vec![directive_context(
                input,
                window,
                ContextOrigin::ExpectedTokens,
                ContextConfidence::Inferred,
            )];
        }

        directive_source_window(input)
            .map(|window| {
                directive_context(
                    input,
                    window,
                    ContextOrigin::SourceWindow,
                    ContextConfidence::Fallback,
                )
            })
            .into_iter()
            .collect()
    }
}

/// Wraps a directive-local window as a generic language-service context.
fn directive_context<'a>(
    input: &LanguageContextInput<'a>,
    window: DirectiveSourceWindow,
    origin: ContextOrigin,
    confidence: ContextConfidence,
) -> LanguageContext<'a> {
    LanguageContext {
        request: input.request,
        rule: window.rule,
        node: window.node,
        token: input.token,
        origin,
        confidence,
        construct_range: window.construct_range,
        focus_range: window.focus_range,
    }
}

impl Completer<DirectiveAtom> for LanguageService {
    type Params<'a> = (&'a LanguageContext<'a>, &'a DirectiveRegistry);

    /// Provides directive completions from already-classified syntax contexts.
    ///
    /// This method matches on generic [`SyntaxRule`] values and reads
    /// `construct_range`/`focus_range`. It should not parse directive names or
    /// arguments from scratch; if a rule is missing, the grammar or context
    /// provider should be made more structural.
    fn completions((context, registry): Self::Params<'_>) -> Vec<EditorCompletion> {
        if !is_directive_context(context) {
            return Vec::new();
        }

        match context.rule {
            SyntaxRule::DirectiveNamespace => {
                let prefix = context_text(context, context.focus_range);
                namespace_completions(registry, prefix.as_ref())
            }
            SyntaxRule::DirectiveMember => {
                let Some(namespace) = directive_namespace_for_context(context) else {
                    return Vec::new();
                };
                let prefix = context_text(context, context.focus_range);
                member_completions(registry, namespace.as_ref(), prefix.as_ref())
            }
            SyntaxRule::DirectiveArgument => {
                let directive = context_text(context, context.construct_range);
                let prefix = context_text(context, context.focus_range);
                if directive_argument_value_context(context) {
                    let Some(argument) = directive_argument_name_for_context(context) else {
                        return Vec::new();
                    };
                    value_completions(
                        registry,
                        directive.as_ref(),
                        argument.as_ref(),
                        prefix.as_ref(),
                    )
                } else {
                    argument_completions(registry, directive.as_ref(), prefix.as_ref())
                }
            }
            _ => Vec::new(),
        }
    }
}

impl ProvidesProjectAssets<DirectiveAtom> for LanguageService {
    type Params<'a> = &'a LanguageServiceRequest<'a>;

    /// Provides the system directive registry for language-service features.
    fn provide(assets: &mut ProjectAssets, _request: Self::Params<'_>) {
        assets.insert(DirectiveRegistry::system());
    }
}

/// Returns whether a context targets a directive completion role owned by this atom.
fn is_directive_context(context: &LanguageContext<'_>) -> bool {
    matches!(
        context.origin,
        ContextOrigin::Cst | ContextOrigin::ExpectedTokens | ContextOrigin::SourceWindow
    ) && matches!(
        context.rule,
        SyntaxRule::DirectiveArgument
            | SyntaxRule::DirectiveMember
            | SyntaxRule::DirectiveNamespace
    )
}

/// Reads a context range from the original source snapshot.
fn context_text<'a>(context: &LanguageContext<'a>, range: TextRange) -> Cow<'a, str> {
    context.request.parse.source.text(range)
}

/// Classifies directive contexts from concrete CST structure.
///
/// This is the preferred path. It relies on the grammar exposing
/// `directive_namespace`, `directive_member`, and `directive_argument` nodes so
/// completion does not need to infer those concepts by splitting strings.
fn directive_cst_window(input: &LanguageContextInput<'_>) -> Option<DirectiveSourceWindow> {
    let directive = input
        .enclosing_rules
        .iter()
        .find(|rule| rule.rule == SyntaxRule::Directive)?;
    let directive_name = direct_child_rule(input, directive.node, SyntaxRule::DirectiveName)?;

    if let Some(argument) = input
        .enclosing_rules
        .iter()
        .find(|rule| rule.rule == SyntaxRule::DirectiveArgument)
    {
        return directive_argument_window(input, directive_name, argument.node);
    }

    if let Some(argument_list) =
        directive_argument_list_window(input, directive.node, directive_name)
    {
        return Some(argument_list);
    }

    if let Some(member) = direct_child_rule(input, directive_name, SyntaxRule::DirectiveMember) {
        return Some(DirectiveSourceWindow {
            construct_range: input.request.parse.tree.nodes[directive_name].range,
            focus_range: clipped_node_range(input, member),
            rule: SyntaxRule::DirectiveMember,
            node: Some(member),
        });
    }

    if let Some(namespace) =
        direct_child_rule(input, directive_name, SyntaxRule::DirectiveNamespace)
    {
        return Some(DirectiveSourceWindow {
            construct_range: input.request.parse.tree.nodes[directive_name].range,
            focus_range: clipped_node_range(input, namespace),
            rule: SyntaxRule::DirectiveNamespace,
            node: Some(namespace),
        });
    }

    directive_source_window(input)
}

/// Classifies malformed directive positions using parser recovery evidence.
///
/// Expected-token evidence proves the parser was actively recovering at the
/// cursor. The range still comes from a bounded source window because there may
/// be no useful CST node for the incomplete child yet.
fn directive_expected_token_window(
    input: &LanguageContextInput<'_>,
) -> Option<DirectiveSourceWindow> {
    if input.expected_tokens.is_empty() {
        return None;
    }

    directive_source_window(input)
}

/// Builds a directive context from a small source window around the cursor.
///
/// This is intentionally the last resort. It only looks backward to the nearest
/// `@` in the current line/block and then emits the same generic syntax-rule
/// contexts as the CST classifier.
fn directive_source_window(input: &LanguageContextInput<'_>) -> Option<DirectiveSourceWindow> {
    let before = input
        .request
        .parse
        .source
        .text(TextRange::new(0, input.request.byte));
    let directive_start = before.as_ref().rfind('@')?;
    if before.as_ref()[directive_start..]
        .chars()
        .any(|character| matches!(character, '{' | '}' | '\n'))
    {
        return None;
    }

    directive_source_window_from_range(
        input,
        TextRange::new(directive_start + 1, input.request.byte),
    )
}

/// Classifies an empty or partially typed argument list after the opening `(`.
fn directive_argument_list_window(
    input: &LanguageContextInput<'_>,
    directive: usize,
    directive_name: usize,
) -> Option<DirectiveSourceWindow> {
    let lpar = direct_child_token(input, directive, SyntaxToken::LPar)?;
    let lpar_range = input.request.parse.tree.nodes[lpar].range;
    if input.request.byte < lpar_range.end as usize {
        return None;
    }

    Some(DirectiveSourceWindow {
        construct_range: input.request.parse.tree.nodes[directive_name].range,
        focus_range: TextRange::new(input.request.byte, input.request.byte),
        rule: SyntaxRule::DirectiveArgument,
        node: None,
    })
}

/// Classifies a concrete directive argument node as name or value completion.
///
/// `construct_range` is the directive name range so completions can resolve the
/// directive definition. `focus_range` is either the argument name prefix or the
/// value prefix after `:`.
fn directive_argument_window(
    input: &LanguageContextInput<'_>,
    directive_name: usize,
    argument: usize,
) -> Option<DirectiveSourceWindow> {
    let argument_range = input.request.parse.tree.nodes[argument].range;
    let name = first_direct_name_token(input, argument)?;
    let colon = direct_child_token(input, argument, SyntaxToken::Colon);
    let focus_range = if let Some(colon) = colon
        && input.request.byte > input.request.parse.tree.nodes[colon].range.end as usize
    {
        TextRange::new(
            input.request.parse.tree.nodes[colon].range.end as usize,
            input.request.byte.min(argument_range.end as usize),
        )
    } else {
        let name_range = input.request.parse.tree.nodes[name].range;
        TextRange::new(
            name_range.start as usize,
            input.request.byte.min(name_range.end as usize),
        )
    };

    Some(DirectiveSourceWindow {
        construct_range: input.request.parse.tree.nodes[directive_name].range,
        focus_range,
        rule: SyntaxRule::DirectiveArgument,
        node: Some(argument),
    })
}

/// Converts a source-window range into a generic directive context.
///
/// This helper exists only for malformed/incomplete text where the CST lacks
/// the structural node that would normally provide the same ranges.
fn directive_source_window_from_range(
    input: &LanguageContextInput<'_>,
    source_range: TextRange,
) -> Option<DirectiveSourceWindow> {
    let source = input.request.parse.source.text(source_range);
    let source = source.as_ref();
    if let Some(open_paren) = source.rfind('(')
        && !source[open_paren + 1..].contains(')')
    {
        let directive_end = source_range.start as usize + open_paren;
        let arguments_start = open_paren + 1;
        let arguments = &source[arguments_start..];
        let current_start = arguments.rfind(',').map_or(0, |comma| comma + 1);
        let current = &arguments[current_start..];
        let trimmed = current.trim_start();
        let trimmed_start =
            source_range.start as usize + arguments_start + current_start + current.len()
                - trimmed.len();
        let prefix_start = trimmed
            .split_once(':')
            .map_or(trimmed_start, |(name, value)| {
                trimmed_start + name.len() + 1 + value.len() - value.trim_start().len()
            });
        return Some(DirectiveSourceWindow {
            construct_range: TextRange::new(source_range.start as usize, directive_end),
            focus_range: TextRange::new(prefix_start, source_range.end as usize),
            rule: SyntaxRule::DirectiveArgument,
            node: None,
        });
    }

    if let Some((namespace, member)) = source.split_once('.') {
        let member_start = source_range.start as usize + namespace.len() + 1;
        return Some(DirectiveSourceWindow {
            construct_range: source_range,
            focus_range: TextRange::new(member_start, member_start + member.len()),
            rule: SyntaxRule::DirectiveMember,
            node: None,
        });
    }

    if source.is_empty() || is_name_prefix(source) {
        return Some(DirectiveSourceWindow {
            construct_range: source_range,
            focus_range: source_range,
            rule: SyntaxRule::DirectiveNamespace,
            node: None,
        });
    }
    None
}

/// Resolves the namespace text for a member-completion context.
///
/// The directive-name grammar guarantees a namespace/member split for CST
/// contexts. Source-window contexts use the same `construct_range` convention.
fn directive_namespace_for_context<'a>(context: &LanguageContext<'a>) -> Option<Cow<'a, str>> {
    let directive = context_text(context, context.construct_range);
    let directive = directive.as_ref();
    if directive.starts_with('.') {
        return Some(Cow::Borrowed("dsql"));
    }
    let namespace = directive.split_once('.')?.0;
    Some(Cow::Owned(namespace.to_string()))
}

/// Returns whether a directive argument context is completing a value.
///
/// CST contexts use the colon token. Source-window contexts fall back to the
/// text between the directive name and the value focus range.
fn directive_argument_value_context(context: &LanguageContext<'_>) -> bool {
    if let Some(node) = context.node
        && let Some(colon) = direct_context_token(context, node, SyntaxToken::Colon)
    {
        return context.request.byte > context.request.parse.tree.nodes[colon].range.end as usize;
    }
    argument_source_prefix(context).is_some_and(|prefix| prefix.contains(':'))
}

/// Reads the argument name associated with a directive value context.
fn directive_argument_name_for_context<'a>(context: &LanguageContext<'a>) -> Option<Cow<'a, str>> {
    if let Some(node) = context.node
        && let Some(name) = first_direct_context_name_token(context, node)
    {
        return Some(context_text(
            context,
            context.request.parse.tree.nodes[name].range,
        ));
    }

    let prefix = argument_source_prefix(context)?;
    let argument = prefix.split_once(':')?.0.trim();
    Some(Cow::Owned(argument.to_string()))
}

/// Returns the source-window text between directive name and argument focus.
fn argument_source_prefix<'a>(context: &LanguageContext<'a>) -> Option<Cow<'a, str>> {
    let start = context.construct_range.end as usize + 1;
    if start > context.focus_range.start as usize {
        return None;
    }
    Some(context_text(
        context,
        TextRange::new(start, context.focus_range.start as usize),
    ))
}

/// Finds a direct child CST rule without descending into nested constructs.
fn direct_child_rule(
    input: &LanguageContextInput<'_>,
    node: usize,
    target: SyntaxRule,
) -> Option<usize> {
    input.request.parse.tree.nodes[node]
        .children
        .iter()
        .copied()
        .find(|child| {
            matches!(
                input.request.parse.tree.nodes[*child].cst_kind,
                CstKind::Rule(rule) if rule == target
            )
        })
}

/// Finds a direct child token in a node from raw context input.
fn direct_child_token(
    input: &LanguageContextInput<'_>,
    node: usize,
    target: SyntaxToken,
) -> Option<usize> {
    input.request.parse.tree.nodes[node]
        .children
        .iter()
        .copied()
        .find(|child| {
            matches!(
                input.request.parse.tree.nodes[*child].cst_kind,
                CstKind::Token(token) if token == target
            )
        })
}

/// Finds a direct child token in a node from a normalized context.
fn direct_context_token(
    context: &LanguageContext<'_>,
    node: usize,
    target: SyntaxToken,
) -> Option<usize> {
    context.request.parse.tree.nodes[node]
        .children
        .iter()
        .copied()
        .find(|child| {
            matches!(
                context.request.parse.tree.nodes[*child].cst_kind,
                CstKind::Token(token) if token == target
            )
        })
}

fn first_direct_name_token(input: &LanguageContextInput<'_>, node: usize) -> Option<usize> {
    direct_child_token(input, node, SyntaxToken::Name)
}

fn first_direct_context_name_token(context: &LanguageContext<'_>, node: usize) -> Option<usize> {
    direct_context_token(context, node, SyntaxToken::Name)
}

/// Clips a CST node range to the cursor for prefix completion.
fn clipped_node_range(input: &LanguageContextInput<'_>, node: usize) -> TextRange {
    let range = input.request.parse.tree.nodes[node].range;
    if input.request.byte < range.start as usize {
        return TextRange::new(input.request.byte, input.request.byte);
    }
    TextRange::new(
        range.start as usize,
        input.request.byte.min(range.end as usize),
    )
}

fn namespace_completions(registry: &DirectiveRegistry, prefix: &str) -> Vec<EditorCompletion> {
    let mut completions = Vec::new();
    let namespaces = registry.namespace_names();
    if namespaces.contains(&"dsql") && ".".starts_with(prefix) {
        completions.push(EditorCompletion {
            label: ".".to_string(),
            kind: EditorCompletionKind::Directive,
            detail: Some("dsql directive shorthand".to_string()),
            insert_text: None,
        });
    }
    for namespace in namespaces
        .into_iter()
        .filter(|namespace| namespace.starts_with(prefix))
    {
        completions.push(EditorCompletion {
            label: namespace.to_string(),
            kind: EditorCompletionKind::Directive,
            detail: Some("directive namespace".to_string()),
            insert_text: None,
        });
    }
    completions
}

fn member_completions(
    registry: &DirectiveRegistry,
    namespace: &str,
    prefix: &str,
) -> Vec<EditorCompletion> {
    registry
        .system_members(namespace)
        .into_iter()
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

fn argument_completions(
    registry: &DirectiveRegistry,
    directive: &str,
    prefix: &str,
) -> Vec<EditorCompletion> {
    let Some(DirectiveDefinition::System(definition)) = registry.resolve_syntax_name(directive)
    else {
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

fn value_completions(
    registry: &DirectiveRegistry,
    directive: &str,
    argument: &str,
    prefix: &str,
) -> Vec<EditorCompletion> {
    let Some(DirectiveDefinition::System(definition)) = registry.resolve_syntax_name(directive)
    else {
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
            .any(|argument| argument == &required_argument.name)
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
