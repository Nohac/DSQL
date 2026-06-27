use crate::language::prelude::*;
use crate::syntax::Selection;
use facet::Facet;

/// Parsed query definition.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct QueryDef {
    pub range: TextRange,
    pub name: Option<NameRef>,
    /// Directives attached to the query definition header.
    pub directives: Vec<Directive>,
    pub selections: Vec<Selection>,
}

impl QueryDef {
    /// Returns the optional query name.
    pub fn name(&self) -> Option<&NameRef> {
        self.name.as_ref()
    }

    /// Iterates selections in the query body.
    pub fn selections(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }
}

/// Language atom that owns query definitions.
pub enum QueryDefAtom {}

/// Lowered query marker produced while query names and children update lowering state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredQueryDef;

language_atom! {
    QueryDefAtom {
        grammar_rule: Rule::QueryDef,
        ast: QueryDef,
        lowered: LoweredQueryDef,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("query definitions are linted by selection-level legacy traversal"),
        variables: required,
        plan: required,
        sql: no_effect("query definitions are planned before SQL generation"),
        metadata: required,
        editor: required,
    }
}

impl BuildsAst<QueryDefAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> QueryDef {
        let names = self.direct_names(node);
        QueryDef {
            range: self.node_range(node),
            name: names.first().cloned(),
            directives: self.directives(node),
            selections: self
                .direct_rule(node, Rule::SelectionSet)
                .map_or_else(Vec::new, |selection_set| self.selection_set(selection_set)),
        }
    }
}

impl Formats<QueryDefAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.write_str("query");
        if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
            self.write_str(" ");
            self.write_str(&name);
        }
        for directive in self.direct_rules(node, SyntaxRule::Directive) {
            self.format_child(directive);
        }
        if let Some(selection_set) = self.direct_rule(node, SyntaxRule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }
}

impl Lowers<QueryDefAtom> for Lowerer {
    fn lower(query: &QueryDef, context: &mut LowerContext<'_>) -> LoweredQueryDef {
        if let Some(name) = &query.name
            && let Some(diagnostic) = context.names.insert_query(name, context.interner)
        {
            context.diagnostics.push(diagnostic);
        }
        for directive in &query.directives {
            context.lower(LowerTarget::Directive(directive));
        }
        crate::semantic::lower_selection_list(&query.selections, context);
        LoweredQueryDef
    }
}

impl Checks<QueryDefAtom> for Checker {
    type Context<'a> = ();

    fn check(_query: &QueryDef, _context: Self::Context<'_>) {}
}

impl ProvidesContext<QueryDefAtom> for LanguageService {
    fn contexts<'a>(_input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        Vec::new()
    }
}

impl Completer<QueryDefAtom> for LanguageService {
    type Params<'a> = ();

    fn completions(_params: Self::Params<'_>) -> Vec<EditorCompletion> {
        Vec::new()
    }
}

impl ProvidesProjectAssets<QueryDefAtom> for LanguageService {
    type Params<'a> = ();

    fn provide(_assets: &mut ProjectAssets, _params: Self::Params<'_>) {}
}

impl InfersVariables<QueryDefAtom> for VariableInference {}

impl Plans<QueryDefAtom> for Planner {}

impl GeneratesMetadata<QueryDefAtom> for MetadataGenerator {}
