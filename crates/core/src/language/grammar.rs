use crate::{
    format::cst::CstFormatter,
    language::context::{LanguageContext, LanguageContextInput},
    language::{atom::AtomDescriptor, atoms::directive::DirectiveAtom},
    language::{
        atom::LanguageAtom,
        stages::{Completer, EditorCompletion, Formats, LanguageService, ProvidesContext},
    },
    syntax::{SyntaxRule, grammar::parser::Rule},
};

type ContextHandler = for<'a> fn(&LanguageContextInput<'a>) -> Vec<LanguageContext<'a>>;
type CompletionHandler = for<'a> fn(&LanguageContext<'a>) -> Vec<EditorCompletion>;
type FormatHandler = for<'a> fn(&mut CstFormatter<'a>, usize);

/// Runtime descriptor for a language atom that can refine cursor contexts.
///
/// This is the erased registry form of [`ProvidesContext`]. Atom files keep the
/// typed implementation; the registry lets the language-service dispatcher ask
/// every registered atom for generic [`LanguageContext`] values without naming
/// the atom that owns the syntax at the cursor.
#[derive(Clone, Copy)]
pub(crate) struct ContextProviderDescriptor {
    contexts: ContextHandler,
}

impl ContextProviderDescriptor {
    const fn new(contexts: ContextHandler) -> Self {
        Self { contexts }
    }

    /// Returns atom-refined contexts derived from raw cursor evidence.
    pub(crate) fn contexts<'a>(self, input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>> {
        (self.contexts)(input)
    }
}

/// Runtime descriptor for a language atom that can provide completions.
///
/// Completion descriptors receive contexts after the provider phase. They
/// should be cheap no-ops when the context rule does not belong to the atom, so
/// the completion dispatcher can simply loop every registered descriptor.
#[derive(Clone, Copy)]
pub struct CompleterDescriptor {
    completions: CompletionHandler,
}

impl CompleterDescriptor {
    const fn new(completions: CompletionHandler) -> Self {
        Self { completions }
    }

    /// Runs this atom's completion provider for the given request.
    pub fn completions(self, context: &LanguageContext<'_>) -> Vec<EditorCompletion> {
        (self.completions)(context)
    }
}

/// Runtime descriptor for a language atom that can format a CST node.
///
/// Formatters are looked up by syntax rule. The caller should not branch on
/// concrete atom names; it should ask [`LanguageAtoms`] for the formatter that
/// owns the current rule and fall back only for legacy syntax.
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
///
/// `LanguageAtoms` is the central source of atom coverage metadata. New grammar
/// rules should be classified here as owned, delegated, legacy, or internal so
/// parser changes cannot silently miss compiler/editor stages.
///
/// Stage consumers should use this registry as their dispatch boundary. A
/// formatter, checker, language-service feature, or future stage should derive
/// the relevant rule/typed key from its traversal and ask `LanguageAtoms` for
/// the matching provider instead of explicitly invoking one atom by name.
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
    ///
    /// The language service is intentionally broadcast-style: it ranks contexts
    /// once, then asks every completer whether it applies. This lets new atoms
    /// add completions without changing the frontend completion dispatcher.
    pub fn completers() -> &'static [CompleterDescriptor] {
        generated::COMPLETERS
    }

    /// Returns all atom context providers registered for the language service.
    ///
    /// Context providers refine raw cursor evidence before any feature-specific
    /// provider runs. Consumers should use these normalized contexts instead of
    /// reimplementing parser or source-window classification.
    pub(crate) fn context_providers() -> &'static [ContextProviderDescriptor] {
        generated::CONTEXT_PROVIDERS
    }

    /// Returns the atom formatter registered for a CST syntax rule, when any.
    ///
    /// This is the model other rule-directed stages should follow: classify the
    /// current syntax, fetch the registered provider, and keep any direct
    /// construct-specific branches as legacy migration code.
    pub(crate) fn formatter_for_syntax_rule(rule: SyntaxRule) -> Option<FormatterDescriptor> {
        generated::formatter_for_rule(rule.into())
    }
}

fn complete<A>(context: &LanguageContext<'_>) -> Vec<EditorCompletion>
where
    A: LanguageAtom,
    LanguageService: Completer<A>,
{
    <LanguageService as Completer<A>>::completions(context)
}

fn contexts<'a, A>(input: &LanguageContextInput<'a>) -> Vec<LanguageContext<'a>>
where
    A: LanguageAtom,
    LanguageService: ProvidesContext<A>,
{
    <LanguageService as ProvidesContext<A>>::contexts(input)
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
                    delegates: [$($delegated:path),* $(,)?] $(,)?
                }
            ),* $(,)?
        }

        legacy: [$($legacy:path),* $(,)?]
        internal: [$($internal:path),* $(,)?]
    ) => {
        mod generated {
            use super::*;

            pub(super) const CONTEXT_PROVIDERS: &[ContextProviderDescriptor] = &[
                $(
                    ContextProviderDescriptor::new(contexts::<$atom>),
                )*
            ];

            pub(super) const COMPLETERS: &[CompleterDescriptor] = &[
                $(
                    CompleterDescriptor::new(complete::<$atom>),
                )*
            ];

            pub(super) fn formatter_for_rule(rule: Rule) -> Option<FormatterDescriptor> {
                match rule {
                    $(
                        $owned => Some(FormatterDescriptor::new(format::<$atom>)),
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
            delegates: [
                Rule::DirectiveArgument,
                Rule::DirectiveMember,
                Rule::DirectiveName,
                Rule::DirectiveNamespace,
            ],
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
