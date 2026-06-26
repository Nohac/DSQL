use crate::syntax::grammar::parser::Rule;

/// Static compiler ownership declaration for one source-level language construct.
///
/// An atom ties one grammar rule to the typed source model and declares that
/// compiler stages have either implemented the construct or made an explicit
/// no-effect decision for it.
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

macro_rules! language_atom {
    (
        $atom:ident {
            grammar_rule: $grammar_rule:path,
            ast: $ast:ty,
            lowered: $lowered:ty,
            build_ast: required,
            format: required,
            lower: required,
            check: required,
            lint: no_effect($lint_reason:literal),
            variables: required,
            plan: required,
            sql: no_effect($sql_reason:literal),
            metadata: required,
            editor: required $(,)?
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
