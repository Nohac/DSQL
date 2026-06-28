use crate::language::prelude::*;
use crate::syntax::{QualifiedNameRef, RelationRef, ScopedPath, ScopedPathSegment};

/// Language atom that owns scoped path nodes.
pub enum ScopedPathAtom {}

language_atom! {
    ScopedPathAtom {
        grammar_rule: Rule::ScopedPath,
        ast: ScopedPath,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("scoped-path validation still runs through centralized semantic traversal"),
        lint: no_effect("scoped paths are linted by centralized semantic traversal"),
        variables: deferred("scoped-path variable inference still runs through centralized semantic traversal"),
        plan: deferred("scoped-path planning still runs through centralized plan construction"),
        sql: no_effect("scoped paths are planned before SQL generation"),
        metadata: deferred("scoped-path metadata still runs through centralized metadata generation"),
        editor: deferred("scoped-path editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<ScopedPathAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> ScopedPath {
        self.scoped_path(node)
    }
}

impl Formats<ScopedPathAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.scoped_path(node);
    }
}

impl Lowers<ScopedPathAtom> for Lowerer {
    fn lower(path: &ScopedPath, context: &mut LowerContext<'_>) {
        for segment in &path.segments {
            <Lowerer as Lowers<ScopedPathSegmentAtom>>::lower(segment, context);
        }
    }
}

deferred_atom_stage_impls!(ScopedPathAtom {
    check: "scoped-path validation still runs through centralized semantic traversal",
    variables: "scoped-path variable inference still runs through centralized semantic traversal",
    plan: "scoped-path planning still runs through centralized plan construction",
    metadata: "scoped-path metadata still runs through centralized metadata generation",
    editor: "scoped-path editor features are not atom-dispatched yet",
});

/// Language atom that owns scoped path segment nodes.
pub enum ScopedPathSegmentAtom {}

language_atom! {
    ScopedPathSegmentAtom {
        grammar_rule: Rule::ScopedPathSegment,
        ast: ScopedPathSegment,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("scoped-path-segment validation still runs through centralized semantic traversal"),
        lint: no_effect("scoped path segments are linted by centralized semantic traversal"),
        variables: deferred("scoped-path-segment variable inference still runs through centralized semantic traversal"),
        plan: deferred("scoped-path-segment planning still runs through centralized plan construction"),
        sql: no_effect("scoped path segments are planned before SQL generation"),
        metadata: deferred("scoped-path-segment metadata still runs through centralized metadata generation"),
        editor: deferred("scoped-path-segment editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<ScopedPathSegmentAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> ScopedPathSegment {
        self.scoped_path_segment(node).unwrap_or_else(|| {
            let missing = self.missing_qualified_name(node);
            ScopedPathSegment {
                range: missing.range,
                schema: missing.schema,
                name: missing.name,
                selector: None,
            }
        })
    }
}

impl Formats<ScopedPathSegmentAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<ScopedPathSegmentAtom> for Lowerer {
    fn lower(segment: &ScopedPathSegment, context: &mut LowerContext<'_>) {
        context.interner.intern(&segment.name.text);
        if let Some(selector) = &segment.selector {
            context.interner.intern(&selector.text);
        }
    }
}

deferred_atom_stage_impls!(ScopedPathSegmentAtom {
    check: "scoped-path-segment validation still runs through centralized semantic traversal",
    variables: "scoped-path-segment variable inference still runs through centralized semantic traversal",
    plan: "scoped-path-segment planning still runs through centralized plan construction",
    metadata: "scoped-path-segment metadata still runs through centralized metadata generation",
    editor: "scoped-path-segment editor features are not atom-dispatched yet",
});

/// Language atom that owns qualified name nodes.
pub enum QualifiedNameAtom {}

language_atom! {
    QualifiedNameAtom {
        grammar_rule: Rule::QualifiedName,
        ast: QualifiedNameRef,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("qualified-name validation still runs through centralized semantic traversal"),
        lint: no_effect("qualified names are linted by centralized semantic traversal"),
        variables: deferred("qualified-name variable inference still runs through centralized semantic traversal"),
        plan: deferred("qualified-name planning still runs through centralized plan construction"),
        sql: no_effect("qualified names are planned before SQL generation"),
        metadata: deferred("qualified-name metadata still runs through centralized metadata generation"),
        editor: deferred("qualified-name editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<QualifiedNameAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> QualifiedNameRef {
        self.qualified_name(node)
            .unwrap_or_else(|| self.missing_qualified_name(node))
    }
}

impl Formats<QualifiedNameAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<QualifiedNameAtom> for Lowerer {
    fn lower(name: &QualifiedNameRef, context: &mut LowerContext<'_>) {
        context.interner.intern(&name.display_text());
    }
}

deferred_atom_stage_impls!(QualifiedNameAtom {
    check: "qualified-name validation still runs through centralized semantic traversal",
    variables: "qualified-name variable inference still runs through centralized semantic traversal",
    plan: "qualified-name planning still runs through centralized plan construction",
    metadata: "qualified-name metadata still runs through centralized metadata generation",
    editor: "qualified-name editor features are not atom-dispatched yet",
});

/// Language atom that owns relation reference nodes.
pub enum RelationRefAtom {}

language_atom! {
    RelationRefAtom {
        grammar_rule: Rule::RelationRef,
        ast: RelationRef,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("relation-reference validation still runs through centralized semantic traversal"),
        lint: no_effect("relation references are linted by centralized semantic traversal"),
        variables: deferred("relation-reference variable inference still runs through centralized semantic traversal"),
        plan: deferred("relation-reference planning still runs through centralized plan construction"),
        sql: no_effect("relation references are planned before SQL generation"),
        metadata: deferred("relation-reference metadata still runs through centralized metadata generation"),
        editor: deferred("relation-reference editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<RelationRefAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> RelationRef {
        self.relation_ref(node)
            .unwrap_or_else(|| self.missing_relation_ref(node))
    }
}

impl Formats<RelationRefAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<RelationRefAtom> for Lowerer {
    fn lower(reference: &RelationRef, context: &mut LowerContext<'_>) {
        context.interner.intern(&reference.display_text());
    }
}

deferred_atom_stage_impls!(RelationRefAtom {
    check: "relation-reference validation still runs through centralized semantic traversal",
    variables: "relation-reference variable inference still runs through centralized semantic traversal",
    plan: "relation-reference planning still runs through centralized plan construction",
    metadata: "relation-reference metadata still runs through centralized metadata generation",
    editor: "relation-reference editor features are not atom-dispatched yet",
});
