use crate::{
    asset::ProjectAssets,
    format::cst::CstFormatter,
    language::context::{
        LanguageContext, LanguageContextInput, LanguageServiceAssetContext, LanguageServiceContext,
    },
    language::params::AtomParam,
    language::{
        atom::AtomDescriptor,
        atoms::{
            clause::{
                ClauseAtom, LimitClauseAtom, OffsetClauseAtom, OrderByClauseAtom, OrderItemAtom,
                SortDirectionAtom, WhereClauseAtom,
            },
            directive::{Directive, DirectiveAtom, DirectiveCheckContext},
            document::{Document, DocumentAtom},
            expression::{
                BinaryExpr, BinaryExprAtom, BinaryOperatorAtom, ComparisonOperatorAtom, ExprAtom,
                LiteralAtom, OperatorVariableAtom, ValueVariableAtom,
            },
            field_selection::{FieldSelection, FieldSelectionAtom},
            fragment_def::{FragmentDef, FragmentDefAtom},
            fragment_spread::{FragmentSpread, FragmentSpreadAtom},
            path::{QualifiedNameAtom, RelationRefAtom, ScopedPathAtom, ScopedPathSegmentAtom},
            query_def::{QueryDef, QueryDefAtom},
            selection::SelectionAtom,
        },
    },
    language::{
        atom::LanguageAtom,
        stages::{
            BuildsAst, CheckContext, CheckTarget, Checker, Checks, Completer, EditorCompletion,
            Formats, LanguageService, LowerContext, Lowerer, Lowers, ProvidesContext,
            ProvidesProjectAssets,
        },
    },
    syntax::grammar::parser::Rule,
    syntax::{
        BinaryOp, BinaryOperator, Clause, Expr, LimitClause, Literal, OffsetClause,
        OperatorVariable, OrderByClause, OrderByItem, QualifiedNameRef, RelationRef, ScopedPath,
        ScopedPathSegment, Selection, SortDirectionExpr, SyntaxRule, ValueVariable, WhereClause,
        grammar::parser::NodeRef, parse::AstBuilder,
    },
};
use derive_more::{From, TryInto};

type ProjectAssetProviderHandler = for<'a> fn(&mut ProjectAssets, &LanguageServiceAssetContext<'a>);
type ContextHandler = for<'a> fn(&LanguageContextInput<'a>) -> Vec<LanguageContext<'a>>;
type CompletionHandler = for<'a> fn(&LanguageServiceContext<'a>) -> Vec<EditorCompletion>;
type FormatHandler = for<'a> fn(&mut CstFormatter<'a>, usize);
type AstBuildHandler = for<'a> fn(&AstBuilder<'a>, NodeRef) -> AstNode;
type LowerHandler = for<'a, 'ctx> fn(AstNodeRef<'a>, &mut LowerContext<'ctx>);
type CheckHandler = for<'a, 'ctx, 'errors> fn(CheckTarget<'a>, &mut CheckContext<'ctx, 'errors>);

/// Typed AST values produced by erased atom AST-build descriptors.
///
/// Parser orchestration uses this enum only at the registry boundary. The
/// grammar declaration wraps each strongly typed [`BuildsAst`] output in the
/// `AstNode` variant declared for that atom.
#[expect(
    dead_code,
    reason = "some atom-built nodes are exposed before every parent traversal consumes them"
)]
pub(crate) enum AstNode {
    Document(Document),
    QueryDef(QueryDef),
    FragmentDef(FragmentDef),
    Directive(Directive),
    FieldSelection(FieldSelection),
    FragmentSpread(FragmentSpread),
    Selection(Selection),
    Clause(Clause),
    WhereClause(WhereClause),
    OrderByClause(OrderByClause),
    LimitClause(LimitClause),
    OffsetClause(OffsetClause),
    OrderItem(OrderByItem),
    SortDirection(SortDirectionExpr),
    Expr(Expr),
    BinaryExpr(BinaryExpr),
    BinaryOperator(BinaryOperator),
    ComparisonOperator(BinaryOp),
    Literal(Literal),
    ValueVariable(ValueVariable),
    OperatorVariable(OperatorVariable),
    ScopedPath(ScopedPath),
    ScopedPathSegment(ScopedPathSegment),
    QualifiedName(QualifiedNameRef),
    RelationRef(RelationRef),
}

/// Borrowed typed AST node passed to semantic atom-stage descriptors.
///
/// This is the lowering counterpart to [`AstNode`]. It carries the typed AST
/// payload while the registry derives atom ownership from the node's grammar
/// rule.
#[derive(From, TryInto)]
pub(crate) enum AstNodeRef<'a> {
    Document(&'a Document),
    QueryDef(&'a QueryDef),
    FragmentDef(&'a FragmentDef),
    Directive(&'a Directive),
    FieldSelection(&'a FieldSelection),
    FragmentSpread(&'a FragmentSpread),
    Selection(&'a Selection),
    Clause(&'a Clause),
    WhereClause(&'a WhereClause),
    OrderByClause(&'a OrderByClause),
    LimitClause(&'a LimitClause),
    OffsetClause(&'a OffsetClause),
    OrderItem(&'a OrderByItem),
    SortDirection(&'a SortDirectionExpr),
    Expr(&'a Expr),
    BinaryExpr(&'a BinaryExpr),
    BinaryOperator(&'a BinaryOperator),
    ComparisonOperator(&'a BinaryOp),
    Literal(&'a Literal),
    ValueVariable(&'a ValueVariable),
    OperatorVariable(&'a OperatorVariable),
    ScopedPath(&'a ScopedPath),
    ScopedPathSegment(&'a ScopedPathSegment),
    QualifiedName(&'a QualifiedNameRef),
    RelationRef(&'a RelationRef),
}

impl AstNodeRef<'_> {
    /// Returns the parser rule for this typed AST node.
    pub(crate) fn rule(&self) -> Rule {
        generated::rule_for_ast_node(self)
    }
}

/// Runtime descriptor for a language atom that can build a typed AST node.
#[derive(Clone, Copy)]
pub(crate) struct AstBuilderDescriptor {
    build: AstBuildHandler,
}

impl AstBuilderDescriptor {
    const fn new(build: AstBuildHandler) -> Self {
        Self { build }
    }

    /// Builds the atom-owned AST node for the given CST node.
    pub(crate) fn build(self, builder: &AstBuilder<'_>, node: NodeRef) -> AstNode {
        (self.build)(builder, node)
    }
}

/// Runtime descriptor for a language atom selected by grammar rule to lower a typed AST target.
#[derive(Clone, Copy)]
pub(crate) struct LowerDescriptor {
    lower: LowerHandler,
}

impl LowerDescriptor {
    const fn new(lower: LowerHandler) -> Self {
        Self { lower }
    }

    /// Runs this atom's lowerer for the given typed AST node.
    pub(crate) fn lower(self, node: AstNodeRef<'_>, context: &mut LowerContext<'_>) {
        (self.lower)(node, context);
    }
}

/// Runtime descriptor for a language atom selected by grammar rule to check a typed target.
#[derive(Clone, Copy)]
pub(crate) struct CheckDescriptor {
    check: CheckHandler,
}

impl CheckDescriptor {
    const fn new(check: CheckHandler) -> Self {
        Self { check }
    }

    /// Runs this atom's checker for the given typed target.
    pub(crate) fn check(self, target: CheckTarget<'_>, context: &mut CheckContext<'_, '_>) {
        (self.check)(target, context);
    }
}

/// Runtime descriptor for a language atom that prepares project assets.
///
/// This is the erased registry form of [`ProvidesProjectAssets`]. Providers run
/// once for the request before ranked contexts are consumed by feature
/// descriptors.
#[derive(Clone, Copy)]
pub(crate) struct ProjectAssetProviderDescriptor {
    provide: ProjectAssetProviderHandler,
}

impl ProjectAssetProviderDescriptor {
    const fn new(provide: ProjectAssetProviderHandler) -> Self {
        Self { provide }
    }

    /// Inserts any project assets this atom contributes for the language-service pass.
    pub(crate) fn provide(
        self,
        assets: &mut ProjectAssets,
        context: &LanguageServiceAssetContext<'_>,
    ) {
        (self.provide)(assets, context);
    }
}

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
    pub fn completions(self, context: &LanguageServiceContext<'_>) -> Vec<EditorCompletion> {
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
}

/// Provider for generated language atom registries and grammar ownership.
///
/// `LanguageAtoms` is the central source of atom coverage metadata. New grammar
/// rules should be classified here as owned or delegated so
/// parser changes cannot silently miss compiler/editor stages.
///
/// Stage consumers should use this registry as their dispatch boundary. A
/// formatter, checker, language-service feature, or future stage should derive
/// the relevant grammar rule from its traversal and ask `LanguageAtoms` for the
/// matching provider instead of explicitly invoking one atom by name.
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

    /// Returns the atom AST builder registered for a grammar rule, when any.
    pub(crate) fn ast_builder_for_rule(rule: Rule) -> Option<AstBuilderDescriptor> {
        generated::ast_builder_for_rule(rule)
    }

    /// Returns all project asset providers registered for the language service.
    ///
    /// The completion dispatcher prepares request-level assets once, then
    /// shares them across all ranked contexts consumed for that request.
    pub(crate) fn project_asset_providers() -> &'static [ProjectAssetProviderDescriptor] {
        generated::PROJECT_ASSET_PROVIDERS
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
    /// construct-specific branches as centralized migration code.
    pub(crate) fn formatter_for_syntax_rule(rule: SyntaxRule) -> Option<FormatterDescriptor> {
        generated::formatter_for_rule(rule.into())
    }

    /// Returns the atom lowerer registered for a grammar rule, when any.
    pub(crate) fn lowerer_for_rule(rule: Rule) -> Option<LowerDescriptor> {
        generated::lowerer_for_rule(rule)
    }

    /// Lowers a typed AST node through the atom that owns its grammar rule.
    pub(crate) fn lower_ast_node(node: AstNodeRef<'_>, context: &mut LowerContext<'_>) {
        if let Some(lowerer) = Self::lowerer_for_rule(node.rule()) {
            lowerer.lower(node, context);
        }
    }

    /// Returns the atom checker registered for a grammar rule, when any.
    pub(crate) fn checker_for_rule(rule: Rule) -> Option<CheckDescriptor> {
        generated::checker_for_rule(rule)
    }
}

impl LowerContext<'_> {
    /// Lowers a typed AST node by looking up its atom descriptor.
    pub(crate) fn lower_ast_node(&mut self, node: AstNodeRef<'_>) {
        LanguageAtoms::lower_ast_node(node, self);
    }
}

impl CheckContext<'_, '_> {
    /// Checks a typed target by looking up its atom descriptor.
    pub(crate) fn check(&mut self, target: CheckTarget<'_>) {
        if let Some(checker) = LanguageAtoms::checker_for_rule(target.rule()) {
            checker.check(target, self);
        }
    }
}

fn provide_project_assets<'a, A>(
    assets: &mut ProjectAssets,
    context: &'a LanguageServiceAssetContext<'a>,
) where
    A: LanguageAtom,
    LanguageService: ProvidesProjectAssets<A>,
{
    let Some(params) = <LanguageService as ProvidesProjectAssets<A>>::Params::extract(context)
    else {
        return;
    };

    <LanguageService as ProvidesProjectAssets<A>>::provide(assets, params);
}

fn complete<'a, A>(context: &'a LanguageServiceContext<'a>) -> Vec<EditorCompletion>
where
    A: LanguageAtom,
    LanguageService: Completer<A>,
{
    let Some(params) = <LanguageService as Completer<A>>::Params::extract(context) else {
        return Vec::new();
    };

    <LanguageService as Completer<A>>::completions(params)
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

fn build_atom_ast<A>(builder: &AstBuilder<'_>, node: NodeRef) -> A::Ast
where
    A: LanguageAtom,
    for<'a> AstBuilder<'a>: BuildsAst<A>,
{
    BuildsAst::<A>::build(builder, node)
}

fn lower<A>(node: AstNodeRef<'_>, context: &mut LowerContext<'_>)
where
    A: LanguageAtom,
    Lowerer: Lowers<A>,
    for<'a> AstNodeRef<'a>: TryInto<&'a A::Ast>,
{
    let Ok(ast) = node.try_into() else {
        return;
    };
    let _ = <Lowerer as Lowers<A>>::lower(ast, context);
}

fn check_directive(target: CheckTarget<'_>, context: &mut CheckContext<'_, '_>) {
    let CheckTarget::Directive {
        directive,
        location,
    } = target;
    <Checker as Checks<DirectiveAtom>>::check(
        directive,
        DirectiveCheckContext {
            registry: context.directive_registry,
            location,
            errors: context.errors,
        },
    );
}

macro_rules! language_grammar {
    (
        $(
            $atom:ident {
                rule: $rule:path,
                ast: AstNode::$ast_variant:ident,
                delegates: [$($delegated:path),* $(,)?] $(,)?
            }
        ),* $(,)?
    ) => {
        mod generated {
            use super::*;

            #[cfg(test)]
            pub(super) const ATOM_RULES: &[(Rule, Rule)] = &[
                $(
                    ($rule, <$atom as LanguageAtom>::GRAMMAR_RULE),
                )*
            ];

            pub(super) const PROJECT_ASSET_PROVIDERS: &[ProjectAssetProviderDescriptor] = &[
                $(
                    ProjectAssetProviderDescriptor::new(provide_project_assets::<$atom>),
                )*
            ];

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
                        $rule => {
                            Some(FormatterDescriptor::new(format::<$atom>))
                        }
                    )*
                    _ => None,
                }
            }

            pub(super) fn ast_builder_for_rule(rule: Rule) -> Option<AstBuilderDescriptor> {
                match rule {
                    $(
                        $rule => {
                            Some(AstBuilderDescriptor::new(|builder, node| {
                                AstNode::$ast_variant(build_atom_ast::<$atom>(builder, node))
                            }))
                        }
                    )*
                    _ => None,
                }
            }

            pub(super) fn lowerer_for_rule(rule: Rule) -> Option<LowerDescriptor> {
                match rule {
                    $(
                        $rule => {
                            Some(LowerDescriptor::new(lower::<$atom>))
                        }
                    )*
                    _ => None,
                }
            }

            pub(super) fn checker_for_rule(rule: Rule) -> Option<CheckDescriptor> {
                match rule {
                    Rule::Directive => Some(CheckDescriptor::new(check_directive)),
                    _ => None,
                }
            }

            pub(super) const fn classify(rule: Rule) -> RuleClassification {
                match rule {
                    $(
                        $rule => {
                            RuleClassification::Owned($atom::DESCRIPTOR)
                        }
                        $(
                            $delegated => RuleClassification::Delegated($atom::DESCRIPTOR),
                        )*
                    )*
                }
            }

            pub(super) fn rule_for_ast_node(node: &AstNodeRef<'_>) -> Rule {
                match node {
                    $(
                        AstNodeRef::$ast_variant(_) => $rule,
                    )*
                }
            }

        }
    };
}

language_grammar! {
    DocumentAtom {
        rule: Rule::Document,
        ast: AstNode::Document,
        delegates: [
            Rule::Definition,
            Rule::Error,
        ],
    },
    QueryDefAtom {
        rule: Rule::QueryDef,
        ast: AstNode::QueryDef,
        delegates: [],
    },
    FragmentDefAtom {
        rule: Rule::FragmentDef,
        ast: AstNode::FragmentDef,
        delegates: [],
    },
    DirectiveAtom {
        rule: Rule::Directive,
        ast: AstNode::Directive,
        delegates: [
            Rule::DirectiveArgument,
            Rule::DirectiveMember,
            Rule::DirectiveName,
            Rule::DirectiveNamespace,
        ],
    },
    FieldSelectionAtom {
        rule: Rule::FieldSelection,
        ast: AstNode::FieldSelection,
        delegates: [
            Rule::FieldSelectionTail,
            Rule::FieldSuffix,
        ],
    },
    FragmentSpreadAtom {
        rule: Rule::FragmentSpread,
        ast: AstNode::FragmentSpread,
        delegates: [],
    },
    SelectionAtom {
        rule: Rule::Selection,
        ast: AstNode::Selection,
        delegates: [
            Rule::SelectionSet,
        ],
    },
    ClauseAtom {
        rule: Rule::Clause,
        ast: AstNode::Clause,
        delegates: [
            Rule::ClauseList,
        ],
    },
    WhereClauseAtom {
        rule: Rule::WhereClause,
        ast: AstNode::WhereClause,
        delegates: [],
    },
    OrderByClauseAtom {
        rule: Rule::OrderByClause,
        ast: AstNode::OrderByClause,
        delegates: [],
    },
    LimitClauseAtom {
        rule: Rule::LimitClause,
        ast: AstNode::LimitClause,
        delegates: [],
    },
    OffsetClauseAtom {
        rule: Rule::OffsetClause,
        ast: AstNode::OffsetClause,
        delegates: [],
    },
    OrderItemAtom {
        rule: Rule::OrderItem,
        ast: AstNode::OrderItem,
        delegates: [],
    },
    SortDirectionAtom {
        rule: Rule::SortDirection,
        ast: AstNode::SortDirection,
        delegates: [],
    },
    ExprAtom {
        rule: Rule::Expr,
        ast: AstNode::Expr,
        delegates: [],
    },
    BinaryExprAtom {
        rule: Rule::BinaryExpr,
        ast: AstNode::BinaryExpr,
        delegates: [],
    },
    BinaryOperatorAtom {
        rule: Rule::BinaryOperator,
        ast: AstNode::BinaryOperator,
        delegates: [],
    },
    ComparisonOperatorAtom {
        rule: Rule::ComparisonOperator,
        ast: AstNode::ComparisonOperator,
        delegates: [],
    },
    LiteralAtom {
        rule: Rule::Literal,
        ast: AstNode::Literal,
        delegates: [],
    },
    ValueVariableAtom {
        rule: Rule::ValueVariable,
        ast: AstNode::ValueVariable,
        delegates: [],
    },
    OperatorVariableAtom {
        rule: Rule::OperatorVariable,
        ast: AstNode::OperatorVariable,
        delegates: [],
    },
    ScopedPathAtom {
        rule: Rule::ScopedPath,
        ast: AstNode::ScopedPath,
        delegates: [],
    },
    ScopedPathSegmentAtom {
        rule: Rule::ScopedPathSegment,
        ast: AstNode::ScopedPathSegment,
        delegates: [],
    },
    QualifiedNameAtom {
        rule: Rule::QualifiedName,
        ast: AstNode::QualifiedName,
        delegates: [],
    },
    RelationRefAtom {
        rule: Rule::RelationRef,
        ast: AstNode::RelationRef,
        delegates: [],
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_rows_match_atom_rule_declarations() {
        for (grammar_rule, atom_rule) in generated::ATOM_RULES {
            assert_eq!(grammar_rule, atom_rule);
        }
    }

    #[test]
    fn migrated_rules_are_atom_owned() {
        let cases = [
            (Rule::BinaryExpr, "BinaryExpr"),
            (Rule::BinaryOperator, "BinaryOperator"),
            (Rule::ComparisonOperator, "BinaryOp"),
            (Rule::Expr, "Expr"),
            (Rule::LimitClause, "LimitClause"),
            (Rule::Literal, "Literal"),
            (Rule::OffsetClause, "OffsetClause"),
            (Rule::OperatorVariable, "OperatorVariable"),
            (Rule::OrderByClause, "OrderByClause"),
            (Rule::OrderItem, "OrderByItem"),
            (Rule::QualifiedName, "QualifiedNameRef"),
            (Rule::RelationRef, "RelationRef"),
            (Rule::ScopedPath, "ScopedPath"),
            (Rule::ScopedPathSegment, "ScopedPathSegment"),
            (Rule::SortDirection, "SortDirectionExpr"),
            (Rule::ValueVariable, "ValueVariable"),
            (Rule::WhereClause, "WhereClause"),
        ];

        for (rule, ast) in cases {
            let RuleClassification::Owned(atom) = LanguageAtoms::classify(rule) else {
                panic!("{rule:?} should be atom-owned");
            };
            assert_eq!(atom.ast, ast);
        }
    }

    #[test]
    fn owned_rules_register_stage_providers() {
        for rule in [
            Rule::Document,
            Rule::QueryDef,
            Rule::FragmentDef,
            Rule::Directive,
            Rule::FieldSelection,
            Rule::FragmentSpread,
            Rule::Selection,
            Rule::Clause,
            Rule::WhereClause,
            Rule::OrderByClause,
            Rule::LimitClause,
            Rule::OffsetClause,
            Rule::OrderItem,
            Rule::SortDirection,
            Rule::Expr,
            Rule::BinaryExpr,
            Rule::BinaryOperator,
            Rule::ComparisonOperator,
            Rule::Literal,
            Rule::ValueVariable,
            Rule::OperatorVariable,
            Rule::ScopedPath,
            Rule::ScopedPathSegment,
            Rule::QualifiedName,
            Rule::RelationRef,
        ] {
            assert!(
                LanguageAtoms::ast_builder_for_rule(rule).is_some(),
                "{rule:?} should register an AST builder"
            );
            assert!(
                LanguageAtoms::lowerer_for_rule(rule).is_some(),
                "{rule:?} should register a lowerer"
            );
        }
    }

    #[test]
    fn structural_rules_are_delegated() {
        assert!(matches!(
            LanguageAtoms::classify(Rule::ClauseList),
            RuleClassification::Delegated(atom) if atom.ast == "Clause"
        ));
        assert!(matches!(
            LanguageAtoms::classify(Rule::SelectionSet),
            RuleClassification::Delegated(atom) if atom.ast == "Selection"
        ));
        assert!(matches!(
            LanguageAtoms::classify(Rule::Definition),
            RuleClassification::Delegated(atom) if atom.ast == "Document"
        ));
        assert!(matches!(
            LanguageAtoms::classify(Rule::Error),
            RuleClassification::Delegated(atom) if atom.ast == "Document"
        ));
    }
}
