use crate::language::prelude::*;
use crate::syntax::{FragmentDef, QueryDef};
use facet::Facet;

/// Parsed DSQL source document.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct Document {
    pub definitions: Vec<Definition>,
}

/// Top-level source definition.
#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum Definition {
    Query(QueryDef),
    Fragment(FragmentDef),
}

/// Parsed source file wrapper used by compiler stages.
#[derive(Clone, Debug, Facet)]
pub struct SourceFile {
    document: Document,
}

impl SourceFile {
    /// Creates a source file from its parsed root document.
    pub fn new(document: Document) -> Self {
        Self { document }
    }

    /// Iterates top-level definitions in source order.
    pub fn definitions(&self) -> impl Iterator<Item = &Definition> {
        self.document.definitions.iter()
    }

    /// Iterates top-level query definitions in source order.
    pub fn queries(&self) -> impl Iterator<Item = &QueryDef> {
        self.document
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Query(query) => Some(query),
                Definition::Fragment(_) => None,
            })
    }

    /// Iterates top-level fragment definitions in source order.
    pub fn fragments(&self) -> impl Iterator<Item = &FragmentDef> {
        self.document
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                Definition::Query(_) => None,
                Definition::Fragment(fragment) => Some(fragment),
            })
    }

    /// Returns the parsed root document.
    pub fn document(&self) -> &Document {
        &self.document
    }
}

/// Language atom that owns the source document root.
pub enum DocumentAtom {}

/// Lowered document marker produced while child definitions update lowering state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredDocument;

language_atom! {
    DocumentAtom {
        grammar_rule: Rule::Document,
        ast: Document,
        lowered: LoweredDocument,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("document nodes route child definitions and do not produce lint diagnostics"),
        variables: required,
        plan: required,
        sql: no_effect("document nodes route child definitions before SQL generation"),
        metadata: required,
        editor: required,
    }
}

impl BuildsAst<DocumentAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> Document {
        let definitions = self
            .descendant_rules(node, &[Rule::QueryDef, Rule::FragmentDef])
            .into_iter()
            .filter_map(|definition| {
                self.build_node(definition).and_then(|built| match built {
                    crate::language::grammar::AstBuildOutput::QueryDef(query) => {
                        Some(Definition::Query(query))
                    }
                    crate::language::grammar::AstBuildOutput::FragmentDef(fragment) => {
                        Some(Definition::Fragment(fragment))
                    }
                    _ => None,
                })
            })
            .collect();
        Document { definitions }
    }
}

impl Formats<DocumentAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let mut first = true;
        for child in self.children(node) {
            match (self.rule(child), self.token(child)) {
                (_, Some(SyntaxToken::Comment)) => {
                    self.blank_between_definitions(&mut first);
                    self.write_range_text(self.node_range(child));
                }
                (Some(SyntaxRule::QueryDef | SyntaxRule::FragmentDef), _) => {
                    self.blank_between_definitions(&mut first);
                    self.format_child(child);
                }
                _ => {}
            }
        }
    }
}

impl Lowers<DocumentAtom> for Lowerer {
    fn lower(document: &Document, context: &mut LowerContext<'_>) -> LoweredDocument {
        for definition in &document.definitions {
            match definition {
                Definition::Query(query) => {
                    context.lower(LowerTarget::QueryDef(query));
                }
                Definition::Fragment(fragment) => {
                    context.lower(LowerTarget::FragmentDef(fragment));
                }
            }
        }
        LoweredDocument
    }
}

impl Checks<DocumentAtom> for Checker {
    type Context<'a> = ();

    fn check(_document: &Document, _context: Self::Context<'_>) {}
}

impl ProvidesContext<DocumentAtom> for LanguageService {
    fn contexts<'a>(_input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        Vec::new()
    }
}

impl Completer<DocumentAtom> for LanguageService {
    type Params<'a> = &'a LanguageContext<'a>;

    fn completions(context: Self::Params<'_>) -> Vec<EditorCompletion> {
        if context.rule != SyntaxRule::Document || !is_document_root_context(context) {
            return Vec::new();
        }

        vec![
            EditorCompletion {
                label: "query".to_string(),
                kind: EditorCompletionKind::Keyword,
                detail: Some("define query".to_string()),
                insert_text: None,
            },
            EditorCompletion {
                label: "fragment".to_string(),
                kind: EditorCompletionKind::Keyword,
                detail: Some("define fragment".to_string()),
                insert_text: None,
            },
        ]
    }
}

fn is_document_root_context(context: &LanguageContext<'_>) -> bool {
    let byte = context.request.byte;
    !context.request.parse.tree.nodes.iter().any(|node| {
        matches!(
            node.cst_kind,
            CstKind::Rule(SyntaxRule::QueryDef | SyntaxRule::FragmentDef)
        ) && node.range.start as usize <= byte
            && byte <= node.range.end as usize
    })
}

impl ProvidesProjectAssets<DocumentAtom> for LanguageService {
    type Params<'a> = ();

    fn provide(_assets: &mut ProjectAssets, _params: Self::Params<'_>) {}
}

impl InfersVariables<DocumentAtom> for VariableInference {}

impl Plans<DocumentAtom> for Planner {}

impl GeneratesMetadata<DocumentAtom> for MetadataGenerator {}
