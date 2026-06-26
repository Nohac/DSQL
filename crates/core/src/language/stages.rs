use crate::{
    language::atom::LanguageAtom,
    language::context::{LanguageContext, LanguageContextInput},
    semantic::{Interner, NameIndex},
    syntax::grammar::parser::NodeRef,
};

/// Builds the typed AST node owned by one language atom.
///
/// This is the typed implementation that an atom descriptor calls after a
/// parser rule has been classified. Callers should prefer descriptor/registry
/// dispatch when they are walking arbitrary syntax.
pub trait BuildsAst<A: LanguageAtom> {
    fn build(&self, node: NodeRef) -> A::Ast;
}

/// Formats the CST node owned by one language atom.
///
/// Formatters are implemented per atom, but stage traversal should reach them
/// through rule lookup on [`crate::language::grammar::LanguageAtoms`].
pub trait Formats<A: LanguageAtom> {
    fn format(&mut self, node: usize);
}

/// Lowers the typed AST owned by one language atom into semantic records.
///
/// Lowering remains context-free. The typed implementation is selected by the
/// stage dispatcher or a future descriptor, not by scattered caller branches.
pub trait Lowers<A: LanguageAtom> {
    fn lower(ast: &A::Ast, interner: &mut Interner, names: &mut NameIndex) -> A::Lowered;
}

/// Checks the typed AST owned by one language atom.
///
/// The checker supplies semantic context such as directive location, visible
/// catalog data, or scoped definition state. The atom implementation validates
/// only the construct it owns.
pub trait Checks<A: LanguageAtom> {
    type Context<'a>;

    fn check(ast: &A::Ast, context: Self::Context<'_>);
}

pub trait NoLintEffect<A: LanguageAtom> {
    const REASON: &'static str;
}

pub trait InfersVariables<A: LanguageAtom> {}

pub trait Plans<A: LanguageAtom> {}

pub trait NoSqlEffect<A: LanguageAtom> {
    const REASON: &'static str;
}

pub trait GeneratesMetadata<A: LanguageAtom> {}

/// Generic completion category produced by compiler atoms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCompletionKind {
    Directive,
    Keyword,
}

/// Generic completion item produced by compiler atoms before frontend mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorCompletion {
    pub label: String,
    pub kind: EditorCompletionKind,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
}

/// Completion support implemented by language atoms for the language service.
///
/// Completion providers consume already-classified [`LanguageContext`] values.
/// They should not rediscover the cursor's syntax role with broad source
/// searches. If the context is too coarse, fix the atom's [`ProvidesContext`]
/// implementation or the grammar rule structure that feeds it.
pub trait Completer<A: LanguageAtom> {
    fn completions(context: &LanguageContext<'_>) -> Vec<EditorCompletion>;
}

/// Cursor-context support implemented by language atoms for the language service.
///
/// Implementations refine [`LanguageContextInput`] into generic syntax-rule
/// contexts with useful ranges. They should prefer CST/expected-token evidence
/// and use bounded source-window recovery only for incomplete parser states.
pub trait ProvidesContext<A: LanguageAtom> {
    fn contexts<'a>(input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>>;
}

pub enum Lowerer {}
pub enum Checker {}
pub enum Linter {}
pub enum VariableInference {}
pub enum Planner {}
pub enum PostgresSqlGenerator {}
pub enum MetadataGenerator {}
pub enum LanguageService {}
