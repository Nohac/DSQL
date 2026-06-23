use crate::language::prelude::*;
use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Directive {
    pub range: TextRange,
    pub name: NameRef,
}

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
        Directive {
            range: self.node_range(node),
            name: self
                .direct_names(node)
                .into_iter()
                .next()
                .unwrap_or_else(|| self.missing_name(node)),
        }
    }
}

impl FormatsAtom<DirectiveAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        if let Some(name) = self.direct_token_text(node, crate::SyntaxToken::Name) {
            self.write_str(" @");
            self.write_str(&name);
        }
    }
}

impl LowersAtom<DirectiveAtom> for Lowerer {
    fn lower(
        directive: &Directive,
        interner: &mut Interner,
        names: &mut NameIndex,
    ) -> LoweredDirective {
        let name = interner.intern(&directive.name.text);
        names.directives.push((name, directive.name.range));
        LoweredDirective { name }
    }
}

impl ChecksAtom<DirectiveAtom> for Checker {}

impl InfersVariablesAtom<DirectiveAtom> for VariableInference {}

impl PlansAtom<DirectiveAtom> for Planner {}

impl GeneratesMetadataAtom<DirectiveAtom> for MetadataGenerator {}

impl ProvidesEditorSupport<DirectiveAtom> for EditorFeatures {}
