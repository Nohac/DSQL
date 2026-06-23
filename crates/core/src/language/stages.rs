use crate::language::atom::LanguageAtom;

pub trait BuildsAst<A: LanguageAtom> {}

pub trait FormatsAtom<A: LanguageAtom> {}

pub trait LowersAtom<A: LanguageAtom> {}

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
