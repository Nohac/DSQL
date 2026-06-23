use crate::{
    language::atom::LanguageAtom,
    semantic::{Interner, NameIndex},
    syntax::grammar::parser::NodeRef,
};

pub trait BuildsAst<A: LanguageAtom> {
    fn build(&self, node: NodeRef) -> A::Ast;
}

pub trait FormatsAtom<A: LanguageAtom> {
    fn format(&mut self, node: usize);
}

pub trait LowersAtom<A: LanguageAtom> {
    fn lower(ast: &A::Ast, interner: &mut Interner, names: &mut NameIndex) -> A::Lowered;
}

pub trait ChecksAtom<A: LanguageAtom> {}

pub trait NoLintEffect<A: LanguageAtom> {
    const REASON: &'static str;
}

pub trait InfersVariablesAtom<A: LanguageAtom> {}

pub trait PlansAtom<A: LanguageAtom> {}

pub trait NoSqlEffect<A: LanguageAtom> {
    const REASON: &'static str;
}

pub trait GeneratesMetadataAtom<A: LanguageAtom> {}

pub trait ProvidesEditorSupport<A: LanguageAtom> {}

pub enum Lowerer {}
pub enum Checker {}
pub enum Linter {}
pub enum VariableInference {}
pub enum Planner {}
pub enum PostgresSqlGenerator {}
pub enum MetadataGenerator {}
pub enum EditorFeatures {}
