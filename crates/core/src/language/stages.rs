use crate::{
    language::atom::LanguageAtom,
    semantic::{Interner, NameIndex},
    syntax::{SourceSnapshot, grammar::parser::NodeRef},
};

pub trait BuildsAst<A: LanguageAtom> {
    fn build(&self, node: NodeRef) -> A::Ast;
}

pub trait Formats<A: LanguageAtom> {
    fn format(&mut self, node: usize);
}

pub trait Lowers<A: LanguageAtom> {
    fn lower(ast: &A::Ast, interner: &mut Interner, names: &mut NameIndex) -> A::Lowered;
}

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

/// Cursor-local source information passed to atoms that provide editor features.
///
/// The frontend owns LSP/protocol mapping, while compiler atoms own language
/// syntax awareness. This request is the boundary between those layers.
#[derive(Clone, Copy)]
pub struct EditorCompletionRequest<'a> {
    pub source: &'a SourceSnapshot,
    pub byte: usize,
}

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
pub trait Completer<A: LanguageAtom> {
    fn completions(request: EditorCompletionRequest<'_>) -> Vec<EditorCompletion>;
}

pub enum Lowerer {}
pub enum Checker {}
pub enum Linter {}
pub enum VariableInference {}
pub enum Planner {}
pub enum PostgresSqlGenerator {}
pub enum MetadataGenerator {}
pub enum LanguageService {}
