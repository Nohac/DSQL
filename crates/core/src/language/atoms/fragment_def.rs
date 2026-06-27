use crate::language::prelude::*;
use crate::syntax::{QualifiedNameRef, Selection};
use facet::Facet;

/// Parsed fragment definition.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct FragmentDef {
    pub range: TextRange,
    pub name: Option<NameRef>,
    pub on: Option<QualifiedNameRef>,
    pub selections: Vec<Selection>,
}

impl FragmentDef {
    /// Returns the optional fragment name.
    pub fn name(&self) -> Option<&NameRef> {
        self.name.as_ref()
    }

    /// Iterates selections in the fragment body.
    pub fn selections(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }
}

/// Language atom that owns fragment definitions.
pub enum FragmentDefAtom {}

/// Lowered fragment marker produced while fragment names and children update lowering state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredFragmentDef;

language_atom! {
    FragmentDefAtom {
        grammar_rule: Rule::FragmentDef,
        ast: FragmentDef,
        lowered: LoweredFragmentDef,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("fragment definitions are linted by selection-level legacy traversal"),
        variables: required,
        plan: required,
        sql: no_effect("fragment definitions are planned before SQL generation"),
        metadata: required,
        editor: required,
    }
}

impl BuildsAst<FragmentDefAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> FragmentDef {
        let names = self.direct_names(node);
        let qualified_names = self.direct_qualified_names(node);
        FragmentDef {
            range: self.node_range(node),
            name: names.first().cloned(),
            on: qualified_names.first().cloned(),
            selections: self
                .direct_rule(node, Rule::SelectionSet)
                .map_or_else(Vec::new, |selection_set| self.selection_set(selection_set)),
        }
    }
}

impl Formats<FragmentDefAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.write_str("fragment");
        let names = self.direct_token_texts(node, SyntaxToken::Name);
        if let Some(name) = names.first() {
            self.write_str(" ");
            self.write_str(name);
        }
        self.write_str(" on");
        if let Some(on) = self.direct_qualified_name_text(node) {
            self.write_str(" ");
            self.write_str(&on);
        }
        if let Some(selection_set) = self.direct_rule(node, SyntaxRule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }
}

impl Lowers<FragmentDefAtom> for Lowerer {
    fn lower(fragment: &FragmentDef, context: &mut LowerContext<'_>) -> LoweredFragmentDef {
        if let Some(name) = &fragment.name
            && let Some(diagnostic) = context.names.insert_fragment(name, context.interner)
        {
            context.diagnostics.push(diagnostic);
        }
        if let Some(on) = &fragment.on {
            context.interner.intern(&on.display_text());
        }
        crate::semantic::lower_selection_list(&fragment.selections, context);
        LoweredFragmentDef
    }
}

impl Checks<FragmentDefAtom> for Checker {
    type Context<'a> = ();

    fn check(_fragment: &FragmentDef, _context: Self::Context<'_>) {}
}

impl ProvidesContext<FragmentDefAtom> for LanguageService {
    fn contexts<'a>(_input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        Vec::new()
    }
}

impl Completer<FragmentDefAtom> for LanguageService {
    type Params<'a> = ();

    fn completions(_params: Self::Params<'_>) -> Vec<EditorCompletion> {
        Vec::new()
    }
}

impl ProvidesProjectAssets<FragmentDefAtom> for LanguageService {
    type Params<'a> = ();

    fn provide(_assets: &mut ProjectAssets, _params: Self::Params<'_>) {}
}

impl InfersVariables<FragmentDefAtom> for VariableInference {}

impl Plans<FragmentDefAtom> for Planner {}

impl GeneratesMetadata<FragmentDefAtom> for MetadataGenerator {}
