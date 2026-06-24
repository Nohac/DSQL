use crate::{
    format::cst::CstFormatter,
    language::{atom::AtomDescriptor, atoms::directive::DirectiveAtom},
    language::{
        atom::LanguageAtom,
        stages::{Completer, EditorCompletion, EditorCompletionRequest, Formats, LanguageService},
    },
    syntax::{SyntaxRule, grammar::parser::Rule},
};

type CompletionHandler = for<'a> fn(EditorCompletionRequest<'a>) -> Vec<EditorCompletion>;
type FormatHandler = for<'a> fn(&mut CstFormatter<'a>, usize);

/// Runtime descriptor for a language atom that can provide completions.
#[derive(Clone, Copy)]
pub struct CompleterDescriptor {
    completions: CompletionHandler,
}

impl CompleterDescriptor {
    const fn new(completions: CompletionHandler) -> Self {
        Self { completions }
    }

    /// Runs this atom's completion provider for the given request.
    pub fn completions(self, request: EditorCompletionRequest<'_>) -> Vec<EditorCompletion> {
        (self.completions)(request)
    }
}

/// Runtime descriptor for a language atom that can format a CST node.
#[derive(Clone, Copy)]
pub(crate) struct FormatterDescriptor {
    format: FormatHandler,
}

impl FormatterDescriptor {
    const fn new(format: FormatHandler) -> Self {
        Self { format }
    }

    /// Runs this atom's formatter for the given CST node.
    pub(crate) fn format(self, formatter: &mut CstFormatter<'_>, node: usize) {
        (self.format)(formatter, node);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleClassification {
    Owned(AtomDescriptor),
    Delegated(AtomDescriptor),
    Legacy,
    Internal,
}

/// Provider for generated language atom registries and grammar ownership.
pub struct LanguageAtoms;

impl LanguageAtoms {
    /// Classifies a generated Lelwel rule by atom ownership.
    ///
    /// The backing match is generated without a wildcard arm, so adding a rule
    /// to `dsql.llw` fails compilation until the rule receives a classification.
    pub const fn classify(rule: Rule) -> RuleClassification {
        generated::classify(rule)
    }

    /// Returns all atom completion providers registered for the language service.
    pub fn completers() -> &'static [CompleterDescriptor] {
        generated::COMPLETERS
    }

    /// Returns the atom formatter registered for a CST syntax rule, when any.
    pub(crate) fn formatter_for_syntax_rule(rule: SyntaxRule) -> Option<FormatterDescriptor> {
        generated::formatter_for_syntax_rule(rule)
    }
}

fn complete<A>(request: EditorCompletionRequest<'_>) -> Vec<EditorCompletion>
where
    A: LanguageAtom,
    LanguageService: Completer<A>,
{
    <LanguageService as Completer<A>>::completions(request)
}

fn format<A>(formatter: &mut CstFormatter<'_>, node: usize)
where
    A: LanguageAtom,
    for<'a> CstFormatter<'a>: Formats<A>,
{
    Formats::<A>::format(formatter, node);
}

macro_rules! language_grammar {
    (
        atoms {
            $(
                $atom:ident {
                    owns: $owned:path,
                    syntax: $syntax:path,
                    delegates: [$($delegated:path),* $(,)?] $(,)?
                }
            ),* $(,)?
        }

        legacy: [$($legacy:path),* $(,)?]
        internal: [$($internal:path),* $(,)?]
    ) => {
        mod generated {
            use super::*;

            pub(super) const COMPLETERS: &[CompleterDescriptor] = &[
                $(
                    CompleterDescriptor::new(complete::<$atom>),
                )*
            ];

            pub(super) fn formatter_for_syntax_rule(
                rule: SyntaxRule,
            ) -> Option<FormatterDescriptor> {
                match rule {
                    $(
                        $syntax => Some(FormatterDescriptor::new(format::<$atom>)),
                    )*
                    _ => None,
                }
            }

            pub(super) const fn classify(rule: Rule) -> RuleClassification {
                match rule {
                    $(
                        $owned => RuleClassification::Owned($atom::DESCRIPTOR),
                        $(
                            $delegated => RuleClassification::Delegated($atom::DESCRIPTOR),
                        )*
                    )*
                    $(
                        $legacy => RuleClassification::Legacy,
                    )*
                    $(
                        $internal => RuleClassification::Internal,
                    )*
                }
            }
        }
    };
}

language_grammar! {
    atoms {
        DirectiveAtom {
            owns: Rule::Directive,
            syntax: SyntaxRule::Directive,
            delegates: [Rule::DirectiveArgument, Rule::DirectiveName],
        },
    }

    legacy: [
        Rule::BinaryExpr,
        Rule::BinaryOperator,
        Rule::Clause,
        Rule::ComparisonOperator,
        Rule::Expr,
        Rule::FieldSelection,
        Rule::FragmentDef,
        Rule::FragmentSpread,
        Rule::LimitClause,
        Rule::Literal,
        Rule::OffsetClause,
        Rule::OperatorVariable,
        Rule::OrderByClause,
        Rule::OrderItem,
        Rule::QualifiedName,
        Rule::QueryDef,
        Rule::RelationRef,
        Rule::ScopedPath,
        Rule::ScopedPathSegment,
        Rule::SortDirection,
        Rule::ValueVariable,
        Rule::WhereClause,
    ]
    internal: [
        Rule::ClauseList,
        Rule::Definition,
        Rule::Document,
        Rule::Error,
        Rule::FieldSelectionTail,
        Rule::FieldSuffix,
        Rule::Selection,
        Rule::SelectionSet,
    ]
}
