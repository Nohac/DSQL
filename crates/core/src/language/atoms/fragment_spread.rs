use crate::language::prelude::*;
use facet::Facet;

/// Parsed fragment spread selection.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct FragmentSpread {
    pub range: TextRange,
    pub name: NameRef,
    pub directives: Vec<Directive>,
}

/// Language atom that owns fragment spread selections.
pub enum FragmentSpreadAtom {}

/// Lowered fragment-spread marker produced while spread children update lowering state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredFragmentSpread;

language_atom! {
    FragmentSpreadAtom {
        grammar_rule: Rule::FragmentSpread,
        ast: FragmentSpread,
        lowered: LoweredFragmentSpread,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("fragment spread linting is still handled by legacy selection traversal"),
        variables: required,
        plan: required,
        sql: no_effect("fragment spreads are expanded before SQL generation"),
        metadata: required,
        editor: required,
    }
}

impl BuildsAst<FragmentSpreadAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> FragmentSpread {
        FragmentSpread {
            range: self.node_range(node),
            name: self
                .direct_names(node)
                .into_iter()
                .next()
                .unwrap_or_else(|| self.missing_name(node)),
            directives: self.directives(node),
        }
    }
}

impl Formats<FragmentSpreadAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
            self.write_str("...");
            self.write_str(&name);
        }
        for directive in self.direct_rules(node, SyntaxRule::Directive) {
            self.format_child(directive);
        }
    }
}

impl Lowers<FragmentSpreadAtom> for Lowerer {
    fn lower(spread: &FragmentSpread, context: &mut LowerContext<'_>) -> LoweredFragmentSpread {
        context.names.fields.push((
            context.interner.intern(&spread.name.text),
            spread.name.range,
        ));
        for directive in &spread.directives {
            context.lower_ast_node(directive.into());
        }
        LoweredFragmentSpread
    }
}

impl Checks<FragmentSpreadAtom> for Checker {
    type Context<'a> = ();

    fn check(_spread: &FragmentSpread, _context: Self::Context<'_>) {}
}

impl ProvidesContext<FragmentSpreadAtom> for LanguageService {
    fn contexts<'a>(_input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        Vec::new()
    }
}

impl Completer<FragmentSpreadAtom> for LanguageService {
    type Params<'a> = ();

    fn completions(_params: Self::Params<'_>) -> Vec<EditorCompletion> {
        Vec::new()
    }
}

impl ProvidesProjectAssets<FragmentSpreadAtom> for LanguageService {
    type Params<'a> = ();

    fn provide(_assets: &mut ProjectAssets, _params: Self::Params<'_>) {}
}

impl InfersVariables<FragmentSpreadAtom> for VariableInference {}

impl Plans<FragmentSpreadAtom> for Planner {}

impl GeneratesMetadata<FragmentSpreadAtom> for MetadataGenerator {}
