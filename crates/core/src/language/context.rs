use crate::{
    language::grammar::{LanguageAtoms, RuleClassification},
    syntax::{CstKind, ParseResult, SyntaxRule, SyntaxToken, TextRange, expected_tokens_at},
};

/// Source position request shared by language-service features.
///
/// The request is intentionally parse-centric. Language-service providers
/// should first use the CST, AST, expected-token data, and byte ranges already
/// produced by parsing before falling back to source-window inspection.
#[derive(Clone, Copy)]
pub struct LanguageServiceRequest<'a> {
    pub parse: &'a ParseResult,
    pub byte: usize,
}

/// Confidence assigned to one normalized language-service context.
///
/// Dispatchers use this to rank provider output. Providers should reserve
/// [`ContextConfidence::Fallback`] for contexts derived from bounded source
/// text rather than concrete CST structure or parser recovery data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ContextConfidence {
    Fallback = 0,
    Inferred = 1,
    Exact = 2,
}

/// Evidence source that produced a normalized language-service context.
///
/// `SourceWindow` does not mean parsing failed. It means a provider used a
/// small cursor-local source slice because the available CST/expected-token
/// evidence was not specific enough for that feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextOrigin {
    Cst,
    ExpectedTokens,
    SourceWindow,
}

/// Enclosing CST rule with atom ownership classification.
///
/// The context provider records these from the smallest enclosing rule outward
/// so atom providers can make local structural decisions without walking the
/// entire syntax tree again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleContext {
    pub rule: SyntaxRule,
    pub node: usize,
    pub classification: RuleClassification,
}

/// Raw cursor evidence used to derive normalized language-service contexts.
///
/// Atom context providers receive this type, not another normalized context.
/// They should refine this evidence into one or more [`LanguageContext`] values
/// with concrete [`SyntaxRule`] targets and useful source ranges.
#[derive(Clone)]
pub struct LanguageContextInput<'a> {
    pub request: LanguageServiceRequest<'a>,
    pub token: Option<usize>,
    pub expected_tokens: Vec<SyntaxToken>,
    pub enclosing_rules: Vec<RuleContext>,
}

/// Cursor-local language context consumed by atom language-service features.
///
/// This is the generic handoff from context classification to completions,
/// hover, definition, and future editor features. `rule` names the syntax role
/// that should handle the cursor. `construct_range` covers the larger syntax
/// construct the feature needs to understand, while `focus_range` covers the
/// precise text being completed or inspected. Atom consumers should read those
/// ranges rather than reparsing raw source strings.
#[derive(Clone)]
pub struct LanguageContext<'a> {
    pub request: LanguageServiceRequest<'a>,
    pub rule: SyntaxRule,
    pub node: Option<usize>,
    pub token: Option<usize>,
    pub origin: ContextOrigin,
    pub confidence: ContextConfidence,
    pub construct_range: TextRange,
    pub focus_range: TextRange,
}

/// Builds ranked language-service contexts for one source position.
///
/// The provider first records generic enclosing CST contexts, then asks atom
/// providers to refine the raw input into more precise contexts. For example,
/// the directive atom turns a generic directive node into
/// `DirectiveNamespace`, `DirectiveMember`, or `DirectiveArgument` contexts.
pub struct LanguageContextProvider;

impl LanguageContextProvider {
    /// Returns ranked contexts for a source position.
    ///
    /// Exact CST contexts are retained for debugging and generic consumers.
    /// Atom-refined contexts are merged into the same list and ranked by
    /// confidence, evidence origin, and CST node specificity.
    pub fn contexts(request: LanguageServiceRequest<'_>) -> Vec<LanguageContext<'_>> {
        let input = LanguageContextInput {
            request,
            token: containing_token(request.parse, request.byte),
            expected_tokens: expected_tokens_at(&request.parse.source, request.byte),
            enclosing_rules: enclosing_rules(request.parse, request.byte),
        };
        let mut contexts = input
            .enclosing_rules
            .iter()
            .map(|rule| LanguageContext {
                request,
                rule: rule.rule,
                node: Some(rule.node),
                token: input.token,
                origin: ContextOrigin::Cst,
                confidence: ContextConfidence::Exact,
                construct_range: request.parse.tree.nodes[rule.node].range,
                focus_range: request.parse.tree.nodes[rule.node].range,
            })
            .collect::<Vec<_>>();

        let atom_contexts = LanguageAtoms::context_providers()
            .iter()
            .flat_map(|provider| provider.contexts(&input))
            .collect::<Vec<_>>();
        contexts.extend(atom_contexts);
        sort_contexts(request.parse, &mut contexts);
        deduplicate_contexts(&mut contexts);

        contexts
    }
}

/// Orders contexts so higher-confidence and more local contexts are consumed first.
fn sort_contexts(parse: &ParseResult, contexts: &mut [LanguageContext<'_>]) {
    contexts.sort_by(|left, right| {
        right
            .confidence
            .cmp(&left.confidence)
            .then_with(|| origin_rank(left.origin).cmp(&origin_rank(right.origin)))
            .then_with(|| node_sort_key(parse, left.node).cmp(&node_sort_key(parse, right.node)))
    });
}

fn origin_rank(origin: ContextOrigin) -> u8 {
    match origin {
        ContextOrigin::Cst => 0,
        ContextOrigin::ExpectedTokens => 1,
        ContextOrigin::SourceWindow => 2,
    }
}

fn node_sort_key(parse: &ParseResult, node: Option<usize>) -> (usize, u32, usize) {
    node.map_or((usize::MAX, u32::MAX, usize::MAX), |node| {
        let range = parse.tree.nodes[node].range;
        (range.len(), range.start, node)
    })
}

/// Removes duplicate contexts after generic and atom-specific providers merge.
fn deduplicate_contexts(contexts: &mut Vec<LanguageContext<'_>>) {
    let mut deduplicated = Vec::new();
    for context in contexts.drain(..) {
        if !deduplicated
            .iter()
            .any(|existing| same_context(existing, &context))
        {
            deduplicated.push(context);
        }
    }
    *contexts = deduplicated;
}

fn same_context(left: &LanguageContext<'_>, right: &LanguageContext<'_>) -> bool {
    left.rule == right.rule
        && left.node == right.node
        && left.token == right.token
        && left.origin == right.origin
        && left.confidence == right.confidence
        && left.construct_range == right.construct_range
        && left.focus_range == right.focus_range
}

fn enclosing_rules(parse: &ParseResult, byte: usize) -> Vec<RuleContext> {
    let mut rules = parse
        .tree
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(node, syntax_node)| {
            let CstKind::Rule(rule) = syntax_node.cst_kind else {
                return None;
            };
            contains_byte(syntax_node.range, byte).then(|| RuleContext {
                rule,
                node,
                classification: LanguageAtoms::classify(rule.into()),
            })
        })
        .collect::<Vec<_>>();
    rules.sort_by_key(|rule| {
        (
            parse.tree.nodes[rule.node].range.len(),
            parse.tree.nodes[rule.node].range.start,
        )
    });
    rules
}

fn containing_token(parse: &ParseResult, byte: usize) -> Option<usize> {
    parse
        .tree
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, syntax_node)| {
            matches!(syntax_node.cst_kind, CstKind::Token(_))
                && contains_byte(syntax_node.range, byte)
        })
        .min_by_key(|(_, syntax_node)| (syntax_node.range.len(), syntax_node.range.start))
        .map(|(node, _)| node)
}

fn contains_byte(range: TextRange, byte: usize) -> bool {
    range.start as usize <= byte && byte <= range.end as usize
}
