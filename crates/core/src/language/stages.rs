use crate::{
    asset::{AtomAssets, ProjectAssets},
    language::atom::LanguageAtom,
    language::context::{
        LanguageContext, LanguageContextInput, LanguageServiceAssetContext, LanguageServiceContext,
    },
    language::params::AtomParam,
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
/// Completion providers declare the narrow typed parameters they need from the
/// full language-service dispatch context. They should not rediscover the
/// cursor's syntax role with broad source searches. If the context is too
/// coarse, fix the atom's [`ProvidesContext`] implementation or the grammar rule
/// structure that feeds it.
pub trait Completer<A: LanguageAtom> {
    type Params<'a>: AtomParam<'a, LanguageServiceContext<'a>>;

    fn completions(params: Self::Params<'_>) -> Vec<EditorCompletion>;
}

/// Cursor-context support implemented by language atoms for the language service.
///
/// Implementations refine [`LanguageContextInput`] into generic syntax-rule
/// contexts with useful ranges. They should prefer CST/expected-token evidence
/// and use bounded source-window recovery only for incomplete parser states.
pub trait ProvidesContext<A: LanguageAtom> {
    fn contexts<'a>(input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>>;
}

/// Project asset preparation implemented by language atoms for the language service.
///
/// Project asset providers run before feature dispatch and populate shared
/// request/project assets. They receive narrow typed parameters extracted from
/// [`LanguageServiceAssetContext`] instead of broad project state.
pub trait ProvidesProjectAssets<A: LanguageAtom> {
    /// Typed parameters this provider needs from the asset-building context.
    type Params<'a>: AtomParam<'a, LanguageServiceAssetContext<'a>>;

    /// Inserts any project assets this atom contributes for the request.
    fn provide(assets: &mut ProjectAssets, params: Self::Params<'_>);
}

/// Per-pass atom asset preparation implemented by language atoms.
///
/// Atom asset providers are for fresh mutable pass-local assets that can be
/// drained after dispatch. They intentionally target [`AtomAssets`] rather than
/// shared project assets.
#[expect(
    dead_code,
    reason = "atom asset providers will be wired when pass-local stage assets are introduced"
)]
pub trait ProvidesAtomAssets<A: LanguageAtom> {
    /// Typed parameters this provider needs from the language-service context.
    type Params<'a>: AtomParam<'a, LanguageServiceContext<'a>>;

    /// Inserts any atom assets this atom contributes for the current pass.
    fn provide(assets: &mut AtomAssets, params: Self::Params<'_>);
}

pub enum Lowerer {}
pub enum Checker {}
pub enum Linter {}
pub enum VariableInference {}
pub enum Planner {}
pub enum PostgresSqlGenerator {}
pub enum MetadataGenerator {}
pub enum LanguageService {}
