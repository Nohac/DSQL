//! Completion service: three layers replace the hand-written cursor
//! classifier the proof of concept carried.
//!
//! 1. **Grammar layer, generated**: the text up to the cursor is parsed and
//!    the parser's structured `expected_tokens()` (a vendored-lelwel patch)
//!    say which tokens may come next; their `literal_text()` spellings
//!    become keyword and operator completions. Grammar changes update this
//!    layer automatically.
//! 2. **Site layer**: the innermost CST ancestors at the cursor classify
//!    the position into a [`CompletionSite`] — a ~30-line match instead of
//!    a thousand-line classifier, because the grammar layer already covers
//!    keyword nuance.
//! 3. **Semantic layer, per entity**: `CompletionStage` systems contribute
//!    [`CompletionCandidate`] facts (columns, relations, fragments) using
//!    the same context-table resolution the checks and hover use.
//!
//! Arbitration merges candidates into the request's [`CompletionList`] in
//! place through a tracked [`RequestKey`] join — one invocation per
//! (request, candidate) pair, each folding its items in at their sorted
//! position. A set-union fold commutes, so pair order is irrelevant and no
//! phase barrier is needed after the candidate systems. Enrichment is an
//! outer join on the file path: resolved requests seed the list with the
//! grammar layer's keywords, unresolved ones keep an empty scaffold list.

use bowl::{
    Bowl, Commands, Component, Entity, Eq as BowlEq, MutRef, Phase, Query, SystemExt, Where, With,
};

use crate::catalog::TableId;
use crate::entities::document::ParsedFile;
use crate::entities::field_selection::{SelectionTree, TreeViews, resolve_field_target};
use crate::grammar::parser::{Node, NodeRef, Parser, Rule};
use crate::service::hover::Position;
use crate::source::{FilePath, ResolutionScope, SourceText};

/// Marks an entity as a completion request; pair with [`FilePath`] and
/// [`Position`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct CompletionRequest;

/// One completion the request may accept.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    /// Text to insert when it differs from the label.
    pub insert_text: Option<String>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionKind {
    Column,
    Relation,
    Table,
    Fragment,
    Scope,
    Operator,
    Keyword,
}

/// The answer, written onto the request entity by the finalizer.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct CompletionList(pub Vec<CompletionItem>);

/// Where the cursor sits, from the innermost CST ancestors. Deliberately
/// coarse: the grammar layer carries the fine distinctions.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum CompletionSite {
    /// Between definitions at the top level.
    DocumentRoot,
    /// Directly inside a query definition's braces: selections name tables.
    RootSelection,
    /// Inside a nested selection set: selections name columns/relations.
    SelectionBody,
    /// Inside a `(...)` clause list but not inside a specific clause.
    ClauseList,
    /// Inside a `where` predicate.
    WhereExpr,
    /// Inside an `order by` item list.
    OrderBy,
    /// On a `...Name` spread name.
    SpreadName,
    /// Somewhere no completions apply beyond grammar keywords.
    Other,
}

/// Enrichment output stamped on the request; entity candidate systems and
/// the finalizer key on it.
#[derive(Debug, Component, Hash)]
#[component(hash)]
pub struct CompletionContext {
    pub site: CompletionSite,
    /// The context table for semantic completions, when one resolves.
    pub table: Option<TableId>,
    /// The requesting file's resolution scope.
    pub scope: String,
    /// Literal spellings of tokens the grammar accepts at the cursor.
    pub keywords: Vec<String>,
}

/// One entity's contribution to one request, addressed by an equal
/// [`RequestKey`].
///
/// [`RequestKey`]: crate::service::hover::RequestKey
#[derive(Component, Hash)]
#[component(hash)]
pub struct CompletionCandidate {
    pub items: Vec<CompletionItem>,
}

pub(crate) async fn register_completion_pipeline(bowl: &Bowl) {
    bowl.add_system(enrich_completion_requests.run_during(Phase::Complete))
        .await;
    bowl.add_system(arbitrate_completions.run_during(Phase::Complete))
        .await;
}

/// Resolves the request's file (bound join on the path), computes the
/// grammar layer from a cursor-truncated parse, the site from the CST, and
/// the context table from the fact tree.
/// The file side of the enrichment outer join: matched per equal path, or
/// `None` exactly once for a request matching no file.
type FileMatch<'a> = Option<
    Query<(Entity, &'a SourceText, &'a ParsedFile, &'a ResolutionScope), Where<BowlEq<FilePath>>>,
>;

async fn enrich_completion_requests(
    requests: Query<(Entity, &FilePath, &Position), With<CompletionRequest>>,
    file: FileMatch<'_>,
    catalog: Query<(Entity, &crate::catalog::CatalogSnapshot)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (request, _path, position) = requests.item();

    // Outer join: an unresolvable file still answers, with an empty list.
    let Some(file) = file else {
        commands
            .entity(request)
            .insert(crate::service::hover::RequestKey(request));
        commands.entity(request).insert(CompletionList(Vec::new()));
        return;
    };

    let (file_entity, _source, parsed, scope) = file.item();
    let (_, snapshot) = catalog.item();

    let offset = position.offset.min(parsed.source.len());

    // Grammar layer: parse the text before the cursor. The vendored
    // expected-token recording captures what could legally come next, both
    // at parse errors and at constructs that close on end of input; only
    // the innermost batch at the cursor position counts — later batches are
    // recovery bubbling outward.
    // The parse snapshot is the settle-consistent text the offset was
    // clamped against; no need to re-materialize the rope.
    let prefix = &parsed.source[..offset];
    let mut parse_diagnostics = Vec::new();
    let truncated = Parser::new(prefix, &mut parse_diagnostics).parse(&mut parse_diagnostics);
    let cursor_error_start = truncated
        .expected_tokens()
        .iter()
        .map(|expected| expected.span.start)
        .max();
    let first_batch_at_cursor = truncated
        .expected_tokens()
        .iter()
        .filter(|expected| Some(expected.span.start) == cursor_error_start)
        .map(|expected| expected.batch)
        .min();
    let mut keywords: Vec<String> = truncated
        .expected_tokens()
        .iter()
        .filter(|expected| {
            Some(expected.span.start) == cursor_error_start
                && Some(expected.batch) == first_batch_at_cursor
        })
        .filter_map(|expected| expected.token.literal_text())
        .map(str::to_string)
        .collect();
    keywords.sort();
    keywords.dedup();

    // Site layer: the rightmost open construct of the truncated tree — the
    // spine reflects what the cursor is inside even when error recovery in
    // the full tree closed the construct early.
    let truncated_cst = truncated.into_data();
    let spine = rightmost_spine(&truncated_cst);
    let site = classify_site(&spine);

    // Semantic layer: the field whose braces or clauses hold the cursor,
    // from the full tree (facts are keyed to its nodes).
    let tree = SelectionTree::collect(&views);
    let table = context_field(&parsed.cst, offset).and_then(|field_node| {
        let key = crate::facts::NodeKey {
            file: file_entity,
            node: field_node.0,
        };
        let (field_entity, _, _, _) = tree.fields.iter().find(|(_, _, k, _)| *k == key)?;
        resolve_field_target(&tree, snapshot.catalog(), *field_entity)
    });

    // Scaffold: the request key and the grammar layer's keyword items.
    // Entity candidates union in through the tracked join; a request whose
    // context yields nothing still answers with this list.
    let mut items = Vec::new();
    for keyword in &keywords {
        let alphabetic = keyword.chars().all(|c| c.is_ascii_alphabetic());
        let comparison = matches!(keyword.as_str(), "==" | "!=" | ">" | ">=" | "<" | "<=");
        if !alphabetic && !comparison {
            // An editor gains nothing from completing `{`.
            continue;
        }
        merge_item(
            &mut items,
            CompletionItem {
                label: keyword.clone(),
                kind: if comparison {
                    CompletionKind::Operator
                } else {
                    CompletionKind::Keyword
                },
                detail: None,
                insert_text: None,
            },
        );
    }
    commands
        .entity(request)
        .insert(crate::service::hover::RequestKey(request));
    commands.entity(request).insert(CompletionList(items));
    commands.entity(request).insert(CompletionContext {
        site,
        table,
        scope: scope.0.clone(),
        keywords,
    });
}

/// Inserts `item` at its sorted (kind, label) position unless the label is
/// already present with an equal-or-lower kind order — an order-independent
/// set union, so arbitration pairs can fold in any order.
fn merge_item(items: &mut Vec<CompletionItem>, item: CompletionItem) {
    if let Some(existing) = items.iter().position(|other| other.label == item.label) {
        if (items[existing].kind, &items[existing].label) <= (item.kind, &item.label) {
            return;
        }
        items.remove(existing);
    }
    let position = items
        .binary_search_by(|other| {
            (other.kind, other.label.clone()).cmp(&(item.kind, item.label.clone()))
        })
        .unwrap_or_else(|position| position);
    items.insert(position, item);
}

/// The rightmost rule chain of the truncated tree: repeatedly descend into
/// the last rule child. The chain ends at the construct still open at the
/// cursor.
fn rightmost_spine(cst: &crate::grammar::parser::CstData) -> Vec<Rule> {
    let mut spine = Vec::new();
    let mut current = NodeRef::ROOT;
    while let Node::Rule(rule, _) = cst.get(current) {
        spine.push(rule);
        let next = cst
            .children(current)
            .filter(|child| matches!(cst.get(*child), Node::Rule(..)))
            .last();
        match next {
            Some(child) => current = child,
            None => break,
        }
    }
    spine
}

fn classify_site(spine: &[Rule]) -> CompletionSite {
    for (index, rule) in spine.iter().enumerate().rev() {
        match rule {
            Rule::WhereClause => return CompletionSite::WhereExpr,
            Rule::OrderByClause => return CompletionSite::OrderBy,
            Rule::ClauseList => return CompletionSite::ClauseList,
            Rule::FragmentSpread => return CompletionSite::SpreadName,
            Rule::SelectionSet => {
                // A selection set directly under a definition lists tables
                // (queries) or the fragment target's fields; deeper ones
                // list the enclosing field's columns and relations.
                let under_field = spine[..index].contains(&Rule::FieldSelection);
                let under_fragment = spine[..index].contains(&Rule::FragmentDef);
                return if under_field || under_fragment {
                    CompletionSite::SelectionBody
                } else {
                    CompletionSite::RootSelection
                };
            }
            _ => {}
        }
    }
    if spine == [Rule::Document] {
        CompletionSite::DocumentRoot
    } else {
        CompletionSite::Other
    }
}

/// The innermost field selection of the full tree containing the cursor
/// (a node whose span ends at the cursor still contains it). Its resolved
/// target is the context table for everything inside it.
fn context_field(cst: &crate::grammar::parser::CstData, offset: usize) -> Option<NodeRef> {
    let mut found = None;
    let mut current = NodeRef::ROOT;
    while let Node::Rule(rule, _) = cst.get(current) {
        if rule == Rule::FieldSelection {
            found = Some(current);
        }
        let next = cst.children(current).find(|child| {
            matches!(cst.get(*child), Node::Rule(..)) && {
                let span = cst.span(*child);
                span.start <= offset && offset <= span.end
            }
        });
        match next {
            Some(child) => current = child,
            None => break,
        }
    }
    found
}

/// Arbitration: one invocation per (request, candidate) pair via the
/// tracked [`RequestKey`] join, each folding the candidate's items into
/// the request's list in place.
///
/// [`RequestKey`]: crate::service::hover::RequestKey
async fn arbitrate_completions(
    query: Query<
        (
            Entity,
            &crate::service::hover::RequestKey,
            MutRef<'_, CompletionList>,
        ),
        With<CompletionRequest>,
    >,
    candidate: Query<
        (Entity, &CompletionCandidate),
        Where<BowlEq<crate::service::hover::RequestKey>>,
    >,
) {
    let (_request, _key, mut list) = query.item();
    let (_candidate_entity, candidate) = candidate.item();

    for item in &candidate.items {
        merge_item(&mut list.0, item.clone());
    }
}
