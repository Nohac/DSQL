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
//! 3. **Semantic layer, per entity**: entity-registered candidate systems contribute
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
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, MutRef, Phase, Query, Registrar,
    SystemExt, View, Where, With,
};

use crate::catalog::TableId;
use crate::entities::document::ParsedFile;
use crate::entities::field_selection::{SelectionTree, TreeViews};
use crate::facts::Span;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{Node, NodeRef, Parser, Rule};
use crate::schema::dsql_schema;
use crate::service::hover::{Position, RequestKey};
use crate::source::{FilePath, ResolutionScope};

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
    Directive,
    Scope,
    Operator,
    Keyword,
}

/// The answer, written onto the request entity by the finalizer.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct CompletionList {
    pub items: Vec<CompletionItem>,
    /// The span of the identifier under the cursor, when there is one:
    /// accepting an item replaces this range instead of appending at the
    /// cursor (mid-word completion).
    pub replace: Option<Span>,
}

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
    /// Inside an aggregate result body.
    AggregateBody,
    /// After `@`, naming a directive namespace (or the `.` shorthand).
    DirectiveName,
    /// After `@namespace.` or `@.`, naming a directive member.
    DirectiveMember,
    /// Inside a directive's parens, naming an argument.
    DirectiveArgument,
    /// After an argument's `:`, supplying its value.
    DirectiveValue,
    /// Somewhere no completions apply beyond grammar keywords.
    Other,
}

/// Enrichment output stamped on the request; entity candidate systems and
/// the finalizer key on it.
#[derive(Debug, Clone, Component, Hash)]
#[component(hash)]
pub struct CompletionContext {
    pub site: CompletionSite,
    /// The context table for semantic completions, when one resolves.
    pub table: Option<TableId>,
    /// The requesting file's resolution scope.
    pub scope: String,
    /// Literal spellings of tokens the grammar accepts at the cursor.
    pub keywords: Vec<String>,
    /// `.` characters immediately before the replaced word, capped at 3:
    /// fragment completions insert only the dots still missing from a
    /// partial `...` spread.
    pub spread_dots: usize,
}

/// Where in a directive the cursor sits, stamped alongside
/// [`CompletionContext`] so the directive entity can contribute items
/// from its registry.
#[derive(Debug, Clone, Component, Hash)]
#[component(hash)]
pub struct DirectiveCompletionContext {
    pub role: DirectiveRole,
}

/// The classified directive position, with the names spelled so far
/// (shorthand already resolved to the dsql namespace).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum DirectiveRole {
    Name,
    Member {
        namespace: String,
    },
    Argument {
        namespace: String,
        member: String,
    },
    Value {
        namespace: String,
        member: String,
        argument: String,
    },
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

/// Emits a non-empty set of completion items for one request.
pub(crate) fn emit_completion_candidate(
    commands: &mut Commands<(dsql_schema::CompletionCandidate,)>,
    request: Entity,
    items: Vec<CompletionItem>,
) {
    if !items.is_empty() {
        commands.insert((
            DerivedFrom::new(request),
            RequestKey(request),
            CompletionCandidate { items },
        ));
    }
}

pub(crate) fn register_completion_pipeline(reg: &mut Registrar<'_>) {
    reg.system(enrich_completion_requests.run_during(Phase::Complete));
    reg.system(arbitrate_completions.run_during(Phase::Complete));
}

/// Resolves the request's file (bound join on the path), computes the
/// grammar layer from a cursor-truncated parse, the site from the CST, and
/// the context table from the fact tree.
#[expect(clippy::too_many_arguments, reason = "system params are injected")]
async fn enrich_completion_requests(
    requests: Query<(Entity, &FilePath, &Position), With<CompletionRequest>>,
    file: crate::service::hover::FileMatch<'_>,
    regions: View<
        '_,
        (
            Entity,
            &crate::source::BelongsToHost,
            &crate::source::SourceOffset,
            &ParsedFile,
        ),
    >,
    documents: View<'_, (Entity, &ParsedFile, &ResolutionScope)>,
    catalog: Query<(Entity, &crate::catalog::CatalogSnapshot)>,
    views: TreeViews<'_>,
    resolutions: View<'_, (Entity, &crate::resolution::ResolvedSelection)>,
    mut commands: Commands<(dsql_schema::CompletionAnswer,)>,
) {
    let (request, _path, position) = requests.item();

    // Outer join: an unresolvable file still answers, with an empty list.
    // A cursor into an embedding host rebases onto the containing region;
    // a host position outside every region has no document either.
    let document = file.map(|file| {
        let (file_entity, _text) = file.item();
        crate::service::hover::map_cursor(&regions, file_entity, position.offset)
    });
    let resolved = document.and_then(|(target, cursor)| {
        documents
            .iter()
            .find(|(entity, _, _)| *entity == target)
            .map(|(_, parsed, scope)| (target, cursor, parsed, scope))
    });
    let Some((file_entity, cursor, parsed, scope)) = resolved else {
        commands
            .entity(request)
            .insert(crate::service::hover::RequestKey(request));
        commands.entity(request).insert(CompletionList {
            items: Vec::new(),
            replace: None,
        });
        return;
    };
    let (_, snapshot) = catalog.item();

    let offset = cursor.min(parsed.source.len());

    // The identifier the cursor touches, when any: items replace it
    // rather than appending mid-word, and its start anchors the layers
    // below so every cursor position within the word answers alike.
    let word = identifier_at(&parsed.source, offset);
    let anchor = word.map_or(offset, |span| span.start);

    // Grammar layer: parse the text before the identifier under the
    // cursor. The vendored expected-token recording captures what could
    // legally come next, both at parse errors and at constructs that
    // close on end of input; only the innermost batch at the cursor
    // position counts — later batches are recovery bubbling outward.
    // The parse snapshot is the settle-consistent text the offset was
    // clamped against; no need to re-materialize the rope.
    let prefix = &parsed.source[..anchor];
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

    // Site and semantic layers share one walk: the open spine of the
    // truncated tree — the constructs still open at the cursor. The spine
    // tracks the position being typed even where error recovery in the
    // full-source tree bails frontier text out to the document, so it
    // works identically for well-formed and mid-edit sources.
    let truncated_cst = truncated.into_data();
    let (spine, stop) = open_spine(&truncated_cst);
    let spread_dots = prefix
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'.')
        .count()
        .min(3);
    let directive = directive_completion(&truncated_cst, &spine, prefix);
    let site = match &directive {
        Some((site, _)) => *site,
        None => match classify_site(&spine, stop) {
            // Dots before the word mean a spread is being typed: only
            // fragments (and the missing dots) make sense, not columns.
            CompletionSite::SelectionBody if spread_dots > 0 => CompletionSite::SpreadName,
            site => site,
        },
    };

    // Semantic layer: the innermost open set or clause decides the context
    // table via its owning construct — a field resolves through its own
    // resolution fact (one semantic decision, made by the resolver), a
    // fragment through its `on` target. Truncated-tree fields map onto
    // resolver facts by their span: both parses see the same tokens before
    // the cursor.
    let tree = SelectionTree::collect(&views);
    let table = match spine_owner(&spine) {
        Some(SetOwner::Field(field_node)) => {
            let field_start = truncated_cst.span(field_node).start;
            tree.fields_by_entity
                .values()
                .find(|(_, field, key, _)| {
                    key.file == file_entity && field.span.start == field_start
                })
                .and_then(|(field_entity, _, _, _)| {
                    resolutions
                        .iter()
                        .find(|(_, resolved)| resolved.field == *field_entity)
                        .and_then(|(_, resolved)| resolved.target.child_context())
                })
        }
        Some(SetOwner::Fragment(def_node)) => {
            crate::entities::direct_rule(&truncated_cst, def_node, Rule::QualifiedName)
                .map(|name| {
                    let span = crate::entities::node_span(&truncated_cst, name);
                    &parsed.source[span.start..span.end]
                })
                .and_then(|name| {
                    snapshot
                        .catalog()
                        .table_ref_for(crate::catalog::TableRef::parse(name))
                })
                .map(|table| table.id)
        }
        Some(SetOwner::Root) | None => None,
    };

    // The replacement span, rebased into the request file's coordinates so
    // embedded-region answers edit the host buffer correctly.
    let rebase = position.offset - cursor;
    let replace = word.map(|span| Span {
        start: span.start + rebase,
        end: span.end + rebase,
    });

    // Scaffold: the request key and the grammar layer's keyword items.
    // Entity candidates union in through the tracked join; a request whose
    // context yields nothing still answers with this list. Sites whose
    // legal continuations are pure names take no grammar items — the
    // expected set there is polluted by the previous construct's own
    // continuations (`where`, operators after a sibling field).
    let mut items = Vec::new();
    let names_only = matches!(
        site,
        CompletionSite::RootSelection
            | CompletionSite::SelectionBody
            | CompletionSite::SpreadName
            | CompletionSite::AggregateBody
            | CompletionSite::DirectiveName
            | CompletionSite::DirectiveMember
            | CompletionSite::DirectiveArgument
            // Values come from the registry (true/false for boolean
            // arguments); generic expression keywords would offer `null`.
            | CompletionSite::DirectiveValue
    );
    for keyword in keywords.iter().filter(|_| !names_only) {
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
    commands
        .entity(request)
        .insert(CompletionList { items, replace });
    if let Some((_, role)) = directive {
        commands
            .entity(request)
            .insert(DirectiveCompletionContext { role });
    }
    commands.entity(request).insert(CompletionContext {
        site,
        table,
        scope: scope.0.clone(),
        keywords,
        spread_dots,
    });
}

/// The span of the identifier the cursor touches — the range completion
/// edits replace. A word ending exactly at the cursor counts (the common
/// type-then-complete position); no word means plain insertion.
fn identifier_at(source: &str, offset: usize) -> Option<Span> {
    // The grammar's identifier shape is [A-Za-z0-9_]; scanning the source
    // is exact and avoids depending on how error recovery shaped the tree.
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let start = source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map(|(at, _)| at);
    let end = source[offset..]
        .char_indices()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map(|(at, c)| offset + at + c.len_utf8());
    match (start, end) {
        (Some(start), Some(end)) => Some(Span { start, end }),
        (Some(start), None) => Some(Span { start, end: offset }),
        (None, Some(end)) => Some(Span { start: offset, end }),
        (None, None) => None,
    }
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
/// Why the open-spine walk stopped before a construct the cursor is not
/// inside of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpineStop {
    /// The construct already has the token that finishes it — a set its
    /// `}`, a clause list its `)`, a spread its name. The cursor sits
    /// *after* it.
    Finished(Rule),
    /// The construct has no tokens yet (recovery opened it for a `{` or
    /// `(` still to be typed). The cursor sits *before* it.
    Unstarted(Rule),
}

/// The last non-trivia token in `node`'s subtree.
fn last_token(cst: &crate::grammar::parser::CstData, node: NodeRef) -> Option<Token> {
    let mut last = None;
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match cst.get(current) {
            Node::Rule(..) => stack.extend(cst.children(current)),
            Node::Token(token, _) => {
                if !matches!(token, Token::Whitespace | Token::Comment) {
                    let span = cst.span(current);
                    if last.is_none_or(|(at, _)| at < span.start) {
                        last = Some((span.start, token));
                    }
                }
            }
        }
    }
    last.map(|(_, token)| token)
}

/// Whether the walk must stop before `node` instead of entering it.
fn spine_stop(
    cst: &crate::grammar::parser::CstData,
    node: NodeRef,
    rule: Rule,
) -> Option<SpineStop> {
    if rule == Rule::Directive {
        let has = |token: Token| {
            cst.children(node)
                .any(|child| matches!(cst.get(child), Node::Token(t, _) if t == token))
        };
        if has(Token::RPar) {
            return Some(SpineStop::Finished(rule));
        }
        if has(Token::LPar) {
            return None;
        }
        // No parens: the directive ends with its name. A name whose last
        // token is a member/namespace Name is complete; a bare `@` or a
        // trailing `.` is still being typed.
        return match last_token(cst, node) {
            Some(Token::Name) => Some(SpineStop::Finished(rule)),
            _ => None,
        };
    }
    let (opener, closer) = match rule {
        Rule::SelectionSet | Rule::AggregateSet => (Token::LBrace, Token::RBrace),
        Rule::ClauseList => (Token::LPar, Token::RPar),
        Rule::FragmentSpread => (Token::Ellipsis, Token::Name),
        _ => return None,
    };
    let has = |token: Token| {
        cst.children(node)
            .any(|child| matches!(cst.get(child), Node::Token(t, _) if t == token))
    };
    if has(closer) {
        Some(SpineStop::Finished(rule))
    } else if !has(opener) {
        Some(SpineStop::Unstarted(rule))
    } else {
        None
    }
}

/// The chain of constructs still open at the end of the truncated parse:
/// the rightmost spine, stopping before any construct that already closed
/// or has not begun. This is the cursor's true containment even mid-edit —
/// error recovery in a full-source parse bails frontier text out to the
/// document, but the truncated parse ends exactly at the cursor, so its
/// open constructs are the ones being typed into.
fn open_spine(cst: &crate::grammar::parser::CstData) -> (Vec<(Rule, NodeRef)>, Option<SpineStop>) {
    let mut spine = Vec::new();
    let mut stop = None;
    let mut current = NodeRef::ROOT;
    while let Node::Rule(rule, _) = cst.get(current) {
        spine.push((rule, current));
        let next = cst
            .children(current)
            .filter(|child| matches!(cst.get(*child), Node::Rule(..)))
            .last();
        let Some(child) = next else { break };
        if let Node::Rule(rule, _) = cst.get(child) {
            stop = spine_stop(cst, child, rule);
            if stop.is_some() {
                break;
            }
        }
        current = child;
    }
    (spine, stop)
}

/// Classifies a cursor inside a directive: which of its zones is being
/// typed, with the names spelled so far (shorthand resolved to `dsql`).
/// `prefix` is the truncated parse's source, so all spans index into it.
fn directive_completion(
    cst: &crate::grammar::parser::CstData,
    spine: &[(Rule, NodeRef)],
    prefix: &str,
) -> Option<(CompletionSite, DirectiveRole)> {
    let (_, directive) = spine
        .iter()
        .rev()
        .find(|(rule, _)| *rule == Rule::Directive)?;
    let child_rule = |parent: NodeRef, rule: Rule| {
        cst.children(parent)
            .find(|child| matches!(cst.get(*child), Node::Rule(r, _) if r == rule))
    };
    let name_text = |parent: NodeRef, rule: Rule| {
        child_rule(parent, rule).and_then(|node| {
            cst.children(node).find_map(|child| match cst.get(child) {
                Node::Token(Token::Name, _) => {
                    let span = cst.span(child);
                    Some(prefix[span.start..span.end].to_string())
                }
                _ => None,
            })
        })
    };

    let name_node = child_rule(*directive, Rule::DirectiveName);
    let namespace = name_node
        .and_then(|name| name_text(name, Rule::DirectiveNamespace))
        .unwrap_or_else(|| crate::entities::directive::DSQL_NAMESPACE.to_string());

    let in_parens = cst
        .children(*directive)
        .any(|child| matches!(cst.get(child), Node::Token(Token::LPar, _)));
    if in_parens {
        let member = name_node
            .and_then(|name| name_text(name, Rule::DirectiveMember))
            .unwrap_or_default();
        // Argument name vs value, structurally: after `(` or `,` a name
        // starts; within an argument only a written `:` commits to the
        // value. (Trivia — including comments — never decides.)
        let last_argument = cst
            .children(*directive)
            .filter(|child| matches!(cst.get(*child), Node::Rule(Rule::DirectiveArgument, _)))
            .last();
        let argument_has_colon = last_argument.is_some_and(|argument| {
            cst.children(argument)
                .any(|child| matches!(cst.get(child), Node::Token(Token::Colon, _)))
        });
        let after_separator = matches!(
            last_token(cst, *directive),
            Some(Token::LPar | Token::Comma)
        );
        if after_separator || !argument_has_colon {
            return Some((
                CompletionSite::DirectiveArgument,
                DirectiveRole::Argument { namespace, member },
            ));
        }
        let argument = last_argument
            .and_then(|argument| {
                cst.children(argument)
                    .find_map(|child| match cst.get(child) {
                        Node::Token(Token::Name, _) => {
                            let span = cst.span(child);
                            Some(prefix[span.start..span.end].to_string())
                        }
                        _ => None,
                    })
            })
            .unwrap_or_default();
        return Some((
            CompletionSite::DirectiveValue,
            DirectiveRole::Value {
                namespace,
                member,
                argument,
            },
        ));
    }

    // Name zone: a `.` commits to a member; before it the namespace (or
    // the shorthand dot) is being typed.
    let has_dot = name_node.is_some_and(|name| {
        cst.children(name)
            .any(|child| matches!(cst.get(child), Node::Token(Token::Dot, _)))
    });
    if has_dot {
        Some((
            CompletionSite::DirectiveMember,
            DirectiveRole::Member { namespace },
        ))
    } else {
        Some((CompletionSite::DirectiveName, DirectiveRole::Name))
    }
}

fn classify_site(spine: &[(Rule, NodeRef)], stop: Option<SpineStop>) -> CompletionSite {
    // A cursor in the gap after a field's `(...)` or in a definition
    // header (before its `{`) has no meaningful completions of its own:
    // the next token is structural.
    if stop == Some(SpineStop::Finished(Rule::ClauseList))
        || stop == Some(SpineStop::Unstarted(Rule::SelectionSet))
    {
        return CompletionSite::Other;
    }
    for (index, (rule, _)) in spine.iter().enumerate().rev() {
        match rule {
            Rule::WhereClause => return CompletionSite::WhereExpr,
            Rule::OrderByClause => return CompletionSite::OrderBy,
            Rule::ClauseList => return CompletionSite::ClauseList,
            Rule::FragmentSpread => return CompletionSite::SpreadName,
            Rule::AggregateSet => return CompletionSite::AggregateBody,
            Rule::SelectionSet => {
                // A selection set directly under a definition lists tables
                // (queries) or the fragment target's fields; deeper ones
                // list the enclosing field's columns and relations.
                let under_field = spine[..index]
                    .iter()
                    .any(|(r, _)| *r == Rule::FieldSelection);
                let under_fragment = spine[..index].iter().any(|(r, _)| *r == Rule::FragmentDef);
                return if under_field || under_fragment {
                    CompletionSite::SelectionBody
                } else {
                    CompletionSite::RootSelection
                };
            }
            _ => {}
        }
    }
    // No construct open: between definitions (the spine stops after a
    // definition's closed body, or the document is empty).
    match spine.last() {
        Some((Rule::Document | Rule::QueryDef | Rule::FragmentDef | Rule::Definition, _)) => {
            CompletionSite::DocumentRoot
        }
        _ => CompletionSite::Other,
    }
}

/// What owns the innermost open set or clause at the cursor.
enum SetOwner {
    /// A field's braces or clauses: complete against the field's resolved
    /// child context.
    Field(NodeRef),
    /// A fragment definition's braces: complete against the `on` target.
    Fragment(NodeRef),
    /// A query definition's braces: root selections name tables.
    Root,
}

/// The owning construct of the innermost open set or clause on the spine.
/// Clause containers count because `title(where .█)` resolves columns
/// against the same table as `title { █ }` — the field's own target.
/// Wrapper rules (`selection`, `field_selection_tail`, …) sit between a
/// container and its owner: the owner is the nearest enclosing field,
/// fragment, or definition.
fn spine_owner(spine: &[(Rule, NodeRef)]) -> Option<SetOwner> {
    let container = spine.iter().rposition(|(rule, _)| {
        matches!(
            rule,
            Rule::SelectionSet
                | Rule::AggregateSet
                | Rule::ClauseList
                | Rule::WhereClause
                | Rule::OrderByClause
        )
    })?;
    spine[..container]
        .iter()
        .rev()
        .find_map(|(rule, node)| match rule {
            Rule::FieldSelection => Some(SetOwner::Field(*node)),
            Rule::FragmentDef => Some(SetOwner::Fragment(*node)),
            Rule::QueryDef => Some(SetOwner::Root),
            _ => None,
        })
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
        merge_item(&mut list.items, item.clone());
    }
}
