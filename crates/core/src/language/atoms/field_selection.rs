use crate::language::prelude::*;
use crate::syntax::{Argument, Clause, RelationRef, Selection};
use facet::Facet;

/// Parsed field selection.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct FieldSelection {
    pub range: TextRange,
    pub alias: Option<NameRef>,
    pub name: RelationRef,
    pub arguments: Vec<Argument>,
    pub has_clause_list: bool,
    pub clauses: Vec<Clause>,
    pub directives: Vec<Directive>,
    pub selections: Vec<Selection>,
}

/// Language atom that owns field selections.
pub enum FieldSelectionAtom {}

/// Lowered field-selection marker produced while field children update lowering state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredFieldSelection;

language_atom! {
    FieldSelectionAtom {
        grammar_rule: Rule::FieldSelection,
        ast: FieldSelection,
        lowered: LoweredFieldSelection,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("field selection linting is still handled by legacy selection traversal"),
        variables: required,
        plan: required,
        sql: no_effect("field selections are planned before SQL generation"),
        metadata: required,
        editor: required,
    }
}

impl BuildsAst<FieldSelectionAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> FieldSelection {
        let first_name = self.direct_relation_refs(node).into_iter().next();
        let tail = self.direct_rule(node, Rule::FieldSelectionTail);
        let (alias, name, suffix) = if let Some(tail) = tail {
            let tail_names = self.direct_relation_refs(tail);
            if tail_names.is_empty() {
                (
                    None,
                    first_name.unwrap_or_else(|| self.missing_relation_ref(node)),
                    self.direct_rule(tail, Rule::FieldSuffix),
                )
            } else {
                (
                    first_name.map(|name| NameRef {
                        range: name.range,
                        text: name.display_text(),
                    }),
                    tail_names
                        .first()
                        .cloned()
                        .unwrap_or_else(|| self.missing_relation_ref(tail)),
                    self.direct_rule(tail, Rule::FieldSuffix),
                )
            }
        } else {
            (
                None,
                first_name.unwrap_or_else(|| self.missing_relation_ref(node)),
                None,
            )
        };
        let clause_list = suffix.and_then(|suffix| self.direct_rule(suffix, Rule::ClauseList));
        let clauses = clause_list.map_or_else(Vec::new, |clauses| self.clauses(clauses));
        let directives = suffix.map_or_else(Vec::new, |suffix| self.directives(suffix));
        let selections = suffix
            .and_then(|suffix| self.direct_rule(suffix, Rule::SelectionSet))
            .map_or_else(Vec::new, |selection_set| self.selection_set(selection_set));

        FieldSelection {
            range: self.node_range(node),
            alias,
            name,
            arguments: Vec::new(),
            has_clause_list: clause_list.is_some(),
            clauses,
            directives,
            selections,
        }
    }
}

impl Formats<FieldSelectionAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let first = self.direct_relation_ref_text(node);
        let tail = self.direct_rule(node, SyntaxRule::FieldSelectionTail);
        let (alias, name, suffix) = if let Some(tail) = tail {
            let tail_name = self.direct_relation_ref_text(tail);
            if tail_name.is_some() {
                (
                    first,
                    tail_name,
                    self.direct_rule(tail, SyntaxRule::FieldSuffix),
                )
            } else {
                (None, first, self.direct_rule(tail, SyntaxRule::FieldSuffix))
            }
        } else {
            (None, first, None)
        };
        if let Some(alias) = alias {
            self.write_str(&alias);
            self.write_str(": ");
        }
        if let Some(name) = name {
            self.write_str(&name);
        }
        if let Some(suffix) = suffix {
            self.field_suffix(suffix);
        }
    }
}

impl Lowers<FieldSelectionAtom> for Lowerer {
    fn lower(selection: &FieldSelection, context: &mut LowerContext<'_>) -> LoweredFieldSelection {
        if let Some(alias) = &selection.alias {
            context.interner.intern(&alias.text);
        }
        context.names.fields.push((
            context.interner.intern(&selection.name.display_text()),
            selection.name.range,
        ));
        for argument in &selection.arguments {
            crate::semantic::lower_argument(argument, context);
        }
        for directive in &selection.directives {
            context.lower_ast_node(directive.into());
        }
        crate::semantic::lower_selection_clauses(&selection.clauses, context);
        crate::semantic::lower_selection_list(&selection.selections, context);
        LoweredFieldSelection
    }
}

impl Checks<FieldSelectionAtom> for Checker {
    type Context<'a> = ();

    fn check(_selection: &FieldSelection, _context: Self::Context<'_>) {}
}

impl ProvidesContext<FieldSelectionAtom> for LanguageService {
    fn contexts<'a>(_input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        Vec::new()
    }
}

impl Completer<FieldSelectionAtom> for LanguageService {
    type Params<'a> = ();

    fn completions(_params: Self::Params<'_>) -> Vec<EditorCompletion> {
        Vec::new()
    }
}

impl ProvidesProjectAssets<FieldSelectionAtom> for LanguageService {
    type Params<'a> = ();

    fn provide(_assets: &mut ProjectAssets, _params: Self::Params<'_>) {}
}

impl InfersVariables<FieldSelectionAtom> for VariableInference {}

impl Plans<FieldSelectionAtom> for Planner {}

impl GeneratesMetadata<FieldSelectionAtom> for MetadataGenerator {}
