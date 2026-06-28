use crate::language::prelude::*;
use crate::syntax::{FieldSelection, Selection};

/// Language atom that owns selection wrapper nodes.
pub enum SelectionAtom {}

language_atom! {
    SelectionAtom {
        grammar_rule: Rule::Selection,
        ast: Selection,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("selection validation still runs through centralized semantic selection traversal"),
        lint: no_effect("selection linting is still handled by centralized selection traversal"),
        variables: deferred("selection variable inference still runs through centralized semantic traversal"),
        plan: deferred("selection planning still runs through centralized plan construction"),
        sql: no_effect("selections are planned before SQL generation"),
        metadata: deferred("selection metadata still runs through centralized metadata generation"),
        editor: deferred("selection editor features still come from field and fragment child atoms"),
    }
}

impl BuildsAst<SelectionAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> Selection {
        self.selection(node).unwrap_or_else(|| {
            Selection::Field(FieldSelection {
                range: self.node_range(node),
                alias: None,
                name: self.missing_relation_ref(node),
                arguments: Vec::new(),
                has_clause_list: false,
                clauses: Vec::new(),
                directives: Vec::new(),
                selections: Vec::new(),
            })
        })
    }
}

impl Formats<SelectionAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.selection(node);
    }
}

impl Lowers<SelectionAtom> for Lowerer {
    fn lower(selection: &Selection, context: &mut LowerContext<'_>) {
        match selection {
            Selection::Field(field) => context.lower_ast_node(field.into()),
            Selection::FragmentSpread(spread) => context.lower_ast_node(spread.into()),
        }
    }
}

deferred_atom_stage_impls!(SelectionAtom {
    check: "selection validation still runs through centralized semantic selection traversal",
    variables: "selection variable inference still runs through centralized semantic traversal",
    plan: "selection planning still runs through centralized plan construction",
    metadata: "selection metadata still runs through centralized metadata generation",
    editor: "selection editor features still come from field and fragment child atoms",
});
