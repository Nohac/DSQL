use crate::syntax::grammar::parser::Rule;

/// Static compiler ownership declaration for one source-level language construct.
///
/// An atom ties one grammar rule to the typed source model and declares that
/// compiler stages have implemented the construct, made an explicit no-effect
/// decision, or recorded a deferred migration owner for it.
pub trait LanguageAtom {
    type Ast;
    type Lowered;

    const GRAMMAR_RULE: Rule;
}

/// Human-readable descriptor emitted by an atom declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtomDescriptor {
    pub grammar_rule: Rule,
    pub ast: &'static str,
    pub lowered: &'static str,
}

macro_rules! atom_deferred_check_coverage {
    ($atom:ty, required) => {};
    ($atom:ty, deferred($reason:literal)) => {
        const _: &str = <crate::language::stages::Checker as crate::language::stages::DeferredCheck<
                                            $atom,
                                        >>::REASON;
    };
}

macro_rules! atom_deferred_variables_coverage {
    ($atom:ty, required) => {};
    ($atom:ty, deferred($reason:literal)) => {
        const _: &str = <crate::language::stages::VariableInference as crate::language::stages::DeferredVariableInference<
            $atom,
        >>::REASON;
    };
}

macro_rules! atom_deferred_plan_coverage {
    ($atom:ty, required) => {};
    ($atom:ty, deferred($reason:literal)) => {
        const _: &str = <crate::language::stages::Planner as crate::language::stages::DeferredPlan<
                                            $atom,
                                        >>::REASON;
    };
}

macro_rules! atom_deferred_metadata_coverage {
    ($atom:ty, required) => {};
    ($atom:ty, deferred($reason:literal)) => {
        const _: &str = <crate::language::stages::MetadataGenerator as crate::language::stages::DeferredMetadata<
            $atom,
        >>::REASON;
    };
}

macro_rules! atom_deferred_editor_coverage {
    ($atom:ty, required) => {};
    ($atom:ty, deferred($reason:literal)) => {
        const _: &str =
            <crate::language::stages::LanguageService as crate::language::stages::DeferredEditor<
                $atom,
            >>::REASON;
    };
}

pub(crate) use atom_deferred_check_coverage;
pub(crate) use atom_deferred_editor_coverage;
pub(crate) use atom_deferred_metadata_coverage;
pub(crate) use atom_deferred_plan_coverage;
pub(crate) use atom_deferred_variables_coverage;

macro_rules! language_atom {
    (
        $atom:ident {
            grammar_rule: $grammar_rule:path,
            ast: $ast:ty,
            lowered: $lowered:ty,
            build_ast: required,
            format: required,
            lower: required,
            check: $check_effect:ident $(($check_reason:literal))?,
            lint: no_effect($lint_reason:literal),
            variables: $variables_effect:ident $(($variables_reason:literal))?,
            plan: $plan_effect:ident $(($plan_reason:literal))?,
            sql: no_effect($sql_reason:literal),
            metadata: $metadata_effect:ident $(($metadata_reason:literal))?,
            editor: $editor_effect:ident $(($editor_reason:literal))? $(,)?
        }
    ) => {
        impl crate::language::atom::LanguageAtom for $atom {
            type Ast = $ast;
            type Lowered = $lowered;

            const GRAMMAR_RULE: crate::syntax::grammar::parser::Rule = $grammar_rule;
        }

        impl crate::language::stages::NoLintEffect<$atom> for crate::language::stages::Linter {
            const REASON: &'static str = $lint_reason;
        }

        impl crate::language::stages::NoSqlEffect<$atom>
            for crate::language::stages::PostgresSqlGenerator
        {
            const REASON: &'static str = $sql_reason;
        }

        impl $atom {
            pub const DESCRIPTOR: crate::language::atom::AtomDescriptor =
                crate::language::atom::AtomDescriptor {
                    grammar_rule: <$atom as crate::language::atom::LanguageAtom>::GRAMMAR_RULE,
                    ast: stringify!($ast),
                    lowered: stringify!($lowered),
                };

            const ATOM_COVERAGE: fn() = || {
                const _: &str =
                    <crate::language::stages::Linter as crate::language::stages::NoLintEffect<
                        $atom,
                    >>::REASON;
                const _: &str = <crate::language::stages::PostgresSqlGenerator as crate::language::stages::NoSqlEffect<
                    $atom,
                >>::REASON;
                $crate::language::atom::atom_deferred_check_coverage!($atom, $check_effect $(($check_reason))?);
                $crate::language::atom::atom_deferred_variables_coverage!($atom, $variables_effect $(($variables_reason))?);
                $crate::language::atom::atom_deferred_plan_coverage!($atom, $plan_effect $(($plan_reason))?);
                $crate::language::atom::atom_deferred_metadata_coverage!($atom, $metadata_effect $(($metadata_reason))?);
                $crate::language::atom::atom_deferred_editor_coverage!($atom, $editor_effect $(($editor_reason))?);

                fn assert_coverage()
                where
                    crate::syntax::parse::AstBuilder<'static>:
                        crate::language::stages::BuildsAst<$atom>,
                    crate::format::cst::CstFormatter<'static>:
                        crate::language::stages::Formats<$atom>,
                    crate::language::stages::Lowerer: crate::language::stages::Lowers<$atom>,
                    crate::language::stages::Checker: crate::language::stages::Checks<$atom>,
                    crate::language::stages::Linter: crate::language::stages::NoLintEffect<$atom>,
                    crate::language::stages::VariableInference:
                        crate::language::stages::InfersVariables<$atom>,
                    crate::language::stages::Planner: crate::language::stages::Plans<$atom>,
                    crate::language::stages::PostgresSqlGenerator:
                        crate::language::stages::NoSqlEffect<$atom>,
                    crate::language::stages::MetadataGenerator:
                        crate::language::stages::GeneratesMetadata<$atom>,
                    crate::language::stages::LanguageService:
                        crate::language::stages::ProvidesContext<$atom>,
                    crate::language::stages::LanguageService:
                        crate::language::stages::Completer<$atom>,
                {
                }

                let _ = assert_coverage;
            };
        }

        const _: fn() = $atom::ATOM_COVERAGE;
    };
}

pub(crate) use language_atom;

macro_rules! deferred_atom_stage_impls {
    (
        $atom:ty {
            check: $check_reason:literal,
            variables: $variables_reason:literal,
            plan: $plan_reason:literal,
            metadata: $metadata_reason:literal,
            editor: $editor_reason:literal $(,)?
        }
    ) => {
        impl crate::language::stages::DeferredCheck<$atom> for crate::language::stages::Checker {
            const REASON: &'static str = $check_reason;
        }

        impl crate::language::stages::Checks<$atom> for crate::language::stages::Checker {
            type Context<'a> = ();

            fn check(
                _ast: &<$atom as crate::language::atom::LanguageAtom>::Ast,
                _context: Self::Context<'_>,
            ) {
            }
        }

        impl crate::language::stages::DeferredEditor<$atom>
            for crate::language::stages::LanguageService
        {
            const REASON: &'static str = $editor_reason;
        }

        impl crate::language::stages::ProvidesContext<$atom>
            for crate::language::stages::LanguageService
        {
            fn contexts<'a>(
                _input: &crate::language::context::LanguageContextInput<'a>,
            ) -> Vec<crate::language::context::LanguageContext<'a>> {
                Vec::new()
            }
        }

        impl crate::language::stages::Completer<$atom>
            for crate::language::stages::LanguageService
        {
            type Params<'a> = ();

            fn completions(
                _params: Self::Params<'_>,
            ) -> Vec<crate::language::stages::EditorCompletion> {
                Vec::new()
            }
        }

        impl crate::language::stages::ProvidesProjectAssets<$atom>
            for crate::language::stages::LanguageService
        {
            type Params<'a> = ();

            fn provide(_assets: &mut crate::asset::ProjectAssets, _params: Self::Params<'_>) {}
        }

        impl crate::language::stages::DeferredVariableInference<$atom>
            for crate::language::stages::VariableInference
        {
            const REASON: &'static str = $variables_reason;
        }

        impl crate::language::stages::InfersVariables<$atom>
            for crate::language::stages::VariableInference
        {
        }

        impl crate::language::stages::DeferredPlan<$atom> for crate::language::stages::Planner {
            const REASON: &'static str = $plan_reason;
        }

        impl crate::language::stages::Plans<$atom> for crate::language::stages::Planner {}

        impl crate::language::stages::DeferredMetadata<$atom>
            for crate::language::stages::MetadataGenerator
        {
            const REASON: &'static str = $metadata_reason;
        }

        impl crate::language::stages::GeneratesMetadata<$atom>
            for crate::language::stages::MetadataGenerator
        {
        }
    };
}

pub(crate) use deferred_atom_stage_impls;
