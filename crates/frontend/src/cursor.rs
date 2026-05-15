use crate::range_contains;
use dsql_core::{
    Catalog, CstKind, DataType, Definition, FieldCheckResult, ParseResult, Selection,
    SelectionKind, SyntaxNode, SyntaxToken, TableId, expected_tokens_at,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CursorContext {
    DocumentRoot,
    FragmentOnKeyword,
    FragmentType,
    RootSelection,
    Invalid,
    FragmentSpread { table: TableId },
    SelectionBody { table: TableId },
    ClauseList { table: TableId, used: UsedClauses },
    WhereScope,
    WhereColumn { table: TableId },
    WhereRelationSelector { table: TableId, relation: String },
    WhereOperator { data_type: DataType },
    OrderByColumn { table: TableId },
    SortDirection,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UsedClauses {
    pub where_clause: bool,
    pub order_by: bool,
    pub limit: bool,
    pub offset: bool,
}

pub(crate) fn cursor_context(parse: &ParseResult, catalog: &Catalog, byte: usize) -> CursorContext {
    if let Some(context) = definition_header_context(parse, byte) {
        return context;
    }

    if let Some(context) = fragment_spread_context(parse, catalog, byte) {
        return context;
    }

    if let Some(context) = clause_context(parse, catalog, byte) {
        return context;
    }

    cst_selection_body_context(parse, catalog, byte)
        .or_else(|| selection_body_context(parse, catalog, byte))
        .unwrap_or(CursorContext::DocumentRoot)
}

fn fragment_spread_context(
    parse: &ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Option<CursorContext> {
    let tokens = parse
        .tree
        .significant_token_nodes_before(byte)
        .collect::<Vec<_>>();
    if tokens.last().and_then(|token| token_kind(token)) != Some(SyntaxToken::Ellipsis) {
        return None;
    }

    match cst_body_table_before(parse, catalog, &tokens) {
        Some(BodyTarget::Table(table)) => Some(CursorContext::FragmentSpread { table }),
        Some(BodyTarget::RootSelection | BodyTarget::Invalid) => Some(CursorContext::Invalid),
        None => None,
    }
}

fn definition_header_context(parse: &ParseResult, byte: usize) -> Option<CursorContext> {
    let tokens = parse
        .tree
        .significant_token_nodes_before(byte)
        .collect::<Vec<_>>();
    if tokens.iter().rev().any(|token| {
        matches!(
            token_kind(token),
            Some(SyntaxToken::LBrace | SyntaxToken::RBrace)
        )
    }) {
        return None;
    }

    let fragment_index = tokens
        .iter()
        .rposition(|token| token_kind(token) == Some(SyntaxToken::Fragment))?;
    let after_fragment = &tokens[fragment_index + 1..];
    let has_name = after_fragment
        .iter()
        .any(|token| token_kind(token) == Some(SyntaxToken::Name));
    if !has_name {
        return None;
    }
    if after_fragment
        .iter()
        .any(|token| token_kind(token) == Some(SyntaxToken::On))
    {
        Some(CursorContext::FragmentType)
    } else {
        Some(CursorContext::FragmentOnKeyword)
    }
}

fn selection_body_context(
    parse: &ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Option<CursorContext> {
    for definition in parse.source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if !range_contains(selection.range, byte) {
                        continue;
                    }
                    let Some(table) = catalog.table_ref(&selection.name.text) else {
                        return Some(CursorContext::Invalid);
                    };
                    return nested_selection_body_context(
                        catalog,
                        table.id,
                        selection,
                        &selection.selections,
                        byte,
                    )
                    .or(Some(CursorContext::SelectionBody { table: table.id }));
                }
            }
            Definition::Fragment(fragment) => {
                if !range_contains(fragment.range, byte) {
                    continue;
                }
                let Some(on) = &fragment.on else {
                    return Some(CursorContext::Invalid);
                };
                let Some(table) = catalog.table_ref(&on.text) else {
                    return Some(CursorContext::Invalid);
                };
                return nested_selection_list_body_context(
                    catalog,
                    table.id,
                    &fragment.selections,
                    byte,
                )
                .or(Some(CursorContext::SelectionBody { table: table.id }));
            }
        }
    }
    None
}

fn nested_selection_body_context(
    catalog: &Catalog,
    parent_table: TableId,
    selection: &Selection,
    selections: &[Selection],
    byte: usize,
) -> Option<CursorContext> {
    match catalog.check_field(parent_table, &selection.name.text) {
        FieldCheckResult::Relation(relation) => {
            nested_selection_list_body_context(catalog, relation.table.id, selections, byte).or(
                Some(CursorContext::SelectionBody {
                    table: relation.table.id,
                }),
            )
        }
        FieldCheckResult::Column(_)
        | FieldCheckResult::NotFound
        | FieldCheckResult::AmbiguousRelation { .. } => Some(CursorContext::Invalid),
    }
}

fn nested_selection_list_body_context(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    byte: usize,
) -> Option<CursorContext> {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread || !range_contains(selection.range, byte)
        {
            continue;
        }
        return nested_selection_body_context(
            catalog,
            table,
            selection,
            &selection.selections,
            byte,
        );
    }
    None
}

fn cst_selection_body_context(
    parse: &ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Option<CursorContext> {
    let tokens = parse
        .tree
        .significant_token_nodes_before(byte)
        .collect::<Vec<_>>();
    let mut stack = Vec::<BodyTarget>::new();

    for (index, token) in tokens.iter().enumerate() {
        match token_kind(token) {
            Some(SyntaxToken::LBrace) => {
                let table = table_for_lbrace(parse, catalog, &tokens, index, stack.last().copied());
                stack.push(table);
            }
            Some(SyntaxToken::RBrace) => {
                stack.pop();
            }
            _ => {}
        }
    }

    match stack.last().copied() {
        Some(BodyTarget::Table(table)) => Some(CursorContext::SelectionBody { table }),
        Some(BodyTarget::RootSelection) => Some(CursorContext::RootSelection),
        Some(BodyTarget::Invalid) => Some(CursorContext::Invalid),
        None => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyTarget {
    RootSelection,
    Table(TableId),
    Invalid,
}

fn table_for_lbrace(
    parse: &ParseResult,
    catalog: &Catalog,
    tokens: &[&SyntaxNode],
    lbrace_index: usize,
    parent: Option<BodyTarget>,
) -> BodyTarget {
    let Some(selection_ref) = selection_ref_before_lbrace(parse, tokens, lbrace_index) else {
        return BodyTarget::Invalid;
    };

    if selection_ref.is_query_body {
        return BodyTarget::RootSelection;
    }

    if let Some(BodyTarget::Table(parent_table)) = parent {
        return match catalog.check_field(parent_table, &selection_ref.name) {
            FieldCheckResult::Relation(relation) => BodyTarget::Table(relation.table.id),
            FieldCheckResult::Column(_)
            | FieldCheckResult::NotFound
            | FieldCheckResult::AmbiguousRelation { .. } => BodyTarget::Invalid,
        };
    }

    catalog
        .table_ref(&selection_ref.name)
        .map_or(BodyTarget::Invalid, |table| BodyTarget::Table(table.id))
}

struct SelectionRef {
    name: String,
    is_query_body: bool,
}

fn selection_ref_before_lbrace(
    parse: &ParseResult,
    tokens: &[&SyntaxNode],
    lbrace_index: usize,
) -> Option<SelectionRef> {
    let previous_index = lbrace_index.checked_sub(1)?;
    match token_kind(tokens[previous_index])? {
        SyntaxToken::Name => {
            let is_query_body = previous_index
                .checked_sub(1)
                .and_then(|index| token_kind(tokens[index]))
                == Some(SyntaxToken::Query);
            Some(SelectionRef {
                name: relation_ref_ending_at(parse, tokens, previous_index)
                    .unwrap_or_else(|| parse.source.text(tokens[previous_index].range).to_string()),
                is_query_body,
            })
        }
        SyntaxToken::RPar => {
            let lpar_index = matching_lpar_before(tokens, previous_index)?;
            table_ref_before_lpar(parse, tokens, lpar_index).map(|name| SelectionRef {
                name,
                is_query_body: false,
            })
        }
        SyntaxToken::LPar => {
            table_ref_before_lpar(parse, tokens, previous_index).map(|name| SelectionRef {
                name,
                is_query_body: false,
            })
        }
        _ => None,
    }
}

fn clause_context(parse: &ParseResult, catalog: &Catalog, byte: usize) -> Option<CursorContext> {
    let cursor = clause_cursor(parse, catalog, byte)?;
    let ClauseTarget::Table(table) = cursor.target else {
        return Some(CursorContext::Invalid);
    };
    let expected = expected_tokens_at(&parse.source, byte);

    if let Some(context) = parsed_clause_context(
        parse,
        catalog,
        table,
        cursor.root_table,
        cursor.parent_table,
        &cursor.suffix_tokens,
    ) {
        return Some(context);
    }

    if expected.contains(&SyntaxToken::Name)
        && let Some(context) = name_expected_context(parse, catalog, &cursor)
    {
        return Some(context);
    }

    if cursor.suffix_tokens.is_empty()
        || expected.iter().any(|token| is_clause_keyword(Some(*token)))
        || expected.is_empty()
    {
        return Some(CursorContext::ClauseList {
            table,
            used: used_clauses_in_tokens(&cursor.suffix_tokens),
        });
    }

    None
}

struct ClauseCursor<'a> {
    target: ClauseTarget,
    root_table: Option<TableId>,
    parent_table: Option<TableId>,
    suffix_tokens: Vec<&'a SyntaxNode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClauseTarget {
    Table(TableId),
    Invalid,
}

fn clause_cursor<'a>(
    parse: &'a ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Option<ClauseCursor<'a>> {
    let tokens = parse
        .tree
        .significant_token_nodes_before(byte)
        .collect::<Vec<_>>();
    let lpar_index = unmatched_lpar_index(&tokens)?;
    let suffix_tokens = tokens[lpar_index + 1..].to_vec();

    if suffix_tokens.iter().any(|token| {
        matches!(
            token_kind(token),
            Some(SyntaxToken::RPar | SyntaxToken::LBrace)
        )
    }) {
        return None;
    }

    let table_ref = table_ref_before_lpar(parse, &tokens, lpar_index)?;
    let ast_target = selection_clause_target(parse, catalog, byte, &table_ref);
    let cst_target = cst_clause_target(parse, catalog, &tokens, lpar_index, &table_ref);
    let root_target = catalog
        .table_ref(&table_ref)
        .map(|table| ClauseTarget::Table(table.id));
    let target = [ast_target, cst_target, root_target]
        .into_iter()
        .flatten()
        .find(|target| matches!(target, ClauseTarget::Table(_)))
        .or_else(|| {
            [ast_target, cst_target, root_target]
                .into_iter()
                .flatten()
                .find(|target| matches!(target, ClauseTarget::Invalid))
        })?;
    let root_table = ast_root_table_at(parse, catalog, byte)
        .or_else(|| cst_root_table_before(parse, catalog, &tokens[..lpar_index]))
        .or_else(|| {
            root_target.and_then(|target| match target {
                ClauseTarget::Table(table) => Some(table),
                ClauseTarget::Invalid => None,
            })
        });
    let parent_table = cst_body_table_before(parse, catalog, &tokens[..lpar_index]).and_then(
        |target| match target {
            BodyTarget::Table(table) => Some(table),
            BodyTarget::RootSelection | BodyTarget::Invalid => None,
        },
    );

    Some(ClauseCursor {
        target,
        root_table,
        parent_table,
        suffix_tokens,
    })
}

fn ast_root_table_at(parse: &ParseResult, catalog: &Catalog, byte: usize) -> Option<TableId> {
    for definition in parse.source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if range_contains(selection.range, byte) {
                        return catalog
                            .table_ref(&selection.name.text)
                            .map(|table| table.id);
                    }
                }
            }
            Definition::Fragment(fragment) => {
                if range_contains(fragment.range, byte) {
                    return fragment
                        .on
                        .as_ref()
                        .and_then(|on| catalog.table_ref(&on.text))
                        .map(|table| table.id);
                }
            }
        }
    }
    None
}

fn parsed_clause_context(
    parse: &ParseResult,
    catalog: &Catalog,
    table: TableId,
    root_table: Option<TableId>,
    parent_table: Option<TableId>,
    suffix_tokens: &[&SyntaxNode],
) -> Option<CursorContext> {
    let clause_index = suffix_tokens
        .iter()
        .rposition(|token| is_clause_keyword(token_kind(token)))?;

    match token_kind(suffix_tokens[clause_index])? {
        SyntaxToken::Where => {
            let after_where = &suffix_tokens[clause_index + 1..];
            if let Some(operator_index) = after_where
                .iter()
                .rposition(|token| is_operator_token(token_kind(token)))
            {
                let after_operator = &after_where[operator_index + 1..];
                if let Some(context) =
                    incomplete_rhs_path_context(parse, catalog, table, root_table, after_operator)
                {
                    return Some(context);
                }
                return after_operator
                    .iter()
                    .any(|token| is_predicate_value_token(token_kind(token)))
                    .then_some(CursorContext::ClauseList {
                        table,
                        used: used_clauses_in_tokens(suffix_tokens),
                    });
            }
            predicate_path_context(parse, catalog, table, root_table, parent_table, after_where)
                .or_else(|| after_where.is_empty().then_some(CursorContext::WhereScope))
        }
        SyntaxToken::Order => {
            if suffix_tokens
                .get(clause_index + 1)
                .and_then(|token| token_kind(token))
                != Some(SyntaxToken::By)
            {
                return Some(CursorContext::ClauseList {
                    table,
                    used: used_clauses_in_tokens(suffix_tokens),
                });
            }
            let after_by = &suffix_tokens[clause_index + 2..];
            let field_count = after_by
                .iter()
                .filter(|token| token_kind(token) == Some(SyntaxToken::Name))
                .count();
            if field_count == 0 {
                Some(CursorContext::OrderByColumn { table })
            } else {
                Some(CursorContext::SortDirection)
            }
        }
        SyntaxToken::Limit | SyntaxToken::Offset => Some(CursorContext::ClauseList {
            table,
            used: used_clauses_in_tokens(suffix_tokens),
        }),
        _ => None,
    }
}

fn name_expected_context(
    parse: &ParseResult,
    catalog: &Catalog,
    cursor: &ClauseCursor<'_>,
) -> Option<CursorContext> {
    let ClauseTarget::Table(table) = cursor.target else {
        return Some(CursorContext::Invalid);
    };
    let clause_index = cursor
        .suffix_tokens
        .iter()
        .rposition(|token| is_clause_keyword(token_kind(token)))?;

    match token_kind(cursor.suffix_tokens[clause_index])? {
        SyntaxToken::Where => {
            let after_where = &cursor.suffix_tokens[clause_index + 1..];
            predicate_path_context(
                parse,
                catalog,
                table,
                cursor.root_table,
                cursor.parent_table,
                after_where,
            )
        }
        SyntaxToken::Order
            if cursor
                .suffix_tokens
                .get(clause_index + 1)
                .and_then(|token| token_kind(token))
                == Some(SyntaxToken::By) =>
        {
            Some(CursorContext::OrderByColumn { table })
        }
        _ => None,
    }
}

fn predicate_path_context(
    parse: &ParseResult,
    catalog: &Catalog,
    current_table: TableId,
    root_table: Option<TableId>,
    parent_table: Option<TableId>,
    after_where: &[&SyntaxNode],
) -> Option<CursorContext> {
    let path = predicate_path_prefix(parse, after_where)?;
    let mut table = match path.scope {
        PredicateCompletionScope::Current => current_table,
        PredicateCompletionScope::Root => root_table?,
        PredicateCompletionScope::Parent => parent_table?,
    };

    if path.segments.is_empty() {
        return Some(CursorContext::WhereColumn { table });
    }

    let relation_segments = if path.trailing_dot {
        path.segments.as_slice()
    } else {
        &path.segments[..path.segments.len().saturating_sub(1)]
    };
    for segment in relation_segments {
        let FieldCheckResult::Relation(relation) = catalog.check_field(table, &segment.field_ref())
        else {
            return None;
        };
        table = relation.table.id;
    }

    if let Some(segment) = path.segments.last()
        && segment.selector_pending
    {
        return Some(CursorContext::WhereRelationSelector {
            table,
            relation: segment.name.clone(),
        });
    }

    if path.trailing_dot {
        return Some(CursorContext::WhereColumn { table });
    }

    let last = path.segments.last()?;
    match catalog.check_field(table, &last.field_ref()) {
        FieldCheckResult::Column(column) => Some(CursorContext::WhereOperator {
            data_type: column.data_type,
        }),
        FieldCheckResult::Relation(_)
        | FieldCheckResult::NotFound
        | FieldCheckResult::AmbiguousRelation { .. } => None,
    }
}

fn incomplete_rhs_path_context(
    parse: &ParseResult,
    catalog: &Catalog,
    table: TableId,
    root_table: Option<TableId>,
    after_operator: &[&SyntaxNode],
) -> Option<CursorContext> {
    if after_operator.is_empty()
        || !matches!(
            token_kind(after_operator.first()?),
            Some(SyntaxToken::Dot | SyntaxToken::DotDot | SyntaxToken::Tilde)
        )
    {
        return None;
    }
    if after_operator
        .iter()
        .any(|token| is_predicate_value_token(token_kind(token)))
    {
        return None;
    }
    predicate_path_context(parse, catalog, table, root_table, None, after_operator)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PredicateCompletionScope {
    Current,
    Parent,
    Root,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PredicatePathPrefix {
    scope: PredicateCompletionScope,
    segments: Vec<PredicatePathSegmentPrefix>,
    trailing_dot: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PredicatePathSegmentPrefix {
    name: String,
    selector: Option<String>,
    selector_pending: bool,
}

impl PredicatePathSegmentPrefix {
    fn field_ref(&self) -> String {
        self.selector.as_ref().map_or_else(
            || self.name.clone(),
            |selector| format!("{}::{}", self.name, selector),
        )
    }
}

fn predicate_path_prefix(
    parse: &ParseResult,
    after_where: &[&SyntaxNode],
) -> Option<PredicatePathPrefix> {
    let scope = match token_kind(after_where.first()?)? {
        SyntaxToken::Dot => PredicateCompletionScope::Current,
        SyntaxToken::DotDot => PredicateCompletionScope::Parent,
        SyntaxToken::Tilde => PredicateCompletionScope::Root,
        _ => return None,
    };

    let mut segments = Vec::new();
    let mut index = 1;
    let mut trailing_dot = false;
    while index < after_where.len() {
        match token_kind(after_where[index])? {
            SyntaxToken::Name => {
                let (segment, next_index) = scoped_path_segment_at(parse, after_where, index)?;
                segments.push(segment);
                index = next_index;
                trailing_dot = false;
            }
            SyntaxToken::Dot => {
                trailing_dot = true;
                index += 1;
            }
            _ => break,
        }
    }

    Some(PredicatePathPrefix {
        scope,
        segments,
        trailing_dot,
    })
}

fn scoped_path_segment_at(
    parse: &ParseResult,
    tokens: &[&SyntaxNode],
    index: usize,
) -> Option<(PredicatePathSegmentPrefix, usize)> {
    if tokens.get(index).and_then(|token| token_kind(token)) != Some(SyntaxToken::Name) {
        return None;
    }
    let name = parse.source.text(tokens[index].range);
    if tokens.get(index + 1).and_then(|token| token_kind(token)) == Some(SyntaxToken::ColonColon) {
        if tokens.get(index + 2).and_then(|token| token_kind(token)) == Some(SyntaxToken::Name) {
            let selector = parse.source.text(tokens[index + 2].range);
            return Some((
                PredicatePathSegmentPrefix {
                    name: name.to_string(),
                    selector: Some(selector.to_string()),
                    selector_pending: false,
                },
                index + 3,
            ));
        }
        return Some((
            PredicatePathSegmentPrefix {
                name: name.to_string(),
                selector: None,
                selector_pending: true,
            },
            index + 2,
        ));
    }
    Some((
        PredicatePathSegmentPrefix {
            name: name.to_string(),
            selector: None,
            selector_pending: false,
        },
        index + 1,
    ))
}

fn selection_clause_target(
    parse: &ParseResult,
    catalog: &Catalog,
    byte: usize,
    field: &str,
) -> Option<ClauseTarget> {
    for definition in parse.source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    let Some(table) = catalog.table_ref(&selection.name.text) else {
                        continue;
                    };
                    if selection.name.text == field && byte >= selection.name.range.end as usize {
                        return Some(ClauseTarget::Table(table.id));
                    }
                    if let Some(target) =
                        nested_clause_target(catalog, table.id, selection, byte, field)
                    {
                        return Some(target);
                    }
                }
            }
            Definition::Fragment(fragment) => {
                let Some(on) = &fragment.on else {
                    continue;
                };
                let Some(table) = catalog.table_ref(&on.text) else {
                    continue;
                };
                for selection in &fragment.selections {
                    if let Some(target) =
                        nested_clause_target(catalog, table.id, selection, byte, field)
                    {
                        return Some(target);
                    }
                }
            }
        }
    }
    None
}

fn nested_clause_target(
    catalog: &Catalog,
    parent_table: TableId,
    selection: &Selection,
    byte: usize,
    field: &str,
) -> Option<ClauseTarget> {
    if selection.kind == SelectionKind::FragmentSpread || !range_contains(selection.range, byte) {
        return None;
    }

    let field_result = catalog.check_field(parent_table, &selection.name.text);
    let current_table = match &field_result {
        FieldCheckResult::Relation(relation) => relation.table.id,
        FieldCheckResult::Column(_)
        | FieldCheckResult::NotFound
        | FieldCheckResult::AmbiguousRelation { .. } => {
            if selection.name.text == field && byte >= selection.name.range.end as usize {
                parent_table
            } else {
                return Some(ClauseTarget::Invalid);
            }
        }
    };

    if selection.name.text == field && byte >= selection.name.range.end as usize {
        return Some(match field_result {
            FieldCheckResult::Relation(relation) => ClauseTarget::Table(relation.table.id),
            FieldCheckResult::Column(_) => ClauseTarget::Invalid,
            FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {
                ClauseTarget::Invalid
            }
        });
    }

    selection
        .selections
        .iter()
        .find_map(|child| nested_clause_target(catalog, current_table, child, byte, field))
}

fn cst_clause_target(
    parse: &ParseResult,
    catalog: &Catalog,
    tokens: &[&SyntaxNode],
    lpar_index: usize,
    field: &str,
) -> Option<ClauseTarget> {
    let parent_table = cst_body_table_before(parse, catalog, &tokens[..lpar_index]);
    if let Some(BodyTarget::Table(parent_table)) = parent_table {
        return Some(match catalog.check_field(parent_table, field) {
            FieldCheckResult::Relation(relation) => ClauseTarget::Table(relation.table.id),
            FieldCheckResult::Column(_) => ClauseTarget::Invalid,
            FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {
                ClauseTarget::Invalid
            }
        });
    }

    catalog
        .table_ref(field)
        .map(|table| ClauseTarget::Table(table.id))
}

fn cst_body_table_before(
    parse: &ParseResult,
    catalog: &Catalog,
    tokens: &[&SyntaxNode],
) -> Option<BodyTarget> {
    let mut stack = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token_kind(token) {
            Some(SyntaxToken::LBrace) => {
                let table = table_for_lbrace(parse, catalog, tokens, index, stack.last().copied());
                stack.push(table);
            }
            Some(SyntaxToken::RBrace) => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.last().copied()
}

fn cst_root_table_before(
    parse: &ParseResult,
    catalog: &Catalog,
    tokens: &[&SyntaxNode],
) -> Option<TableId> {
    let mut stack = Vec::new();
    let mut root_table = None;
    for (index, token) in tokens.iter().enumerate() {
        match token_kind(token) {
            Some(SyntaxToken::LBrace) => {
                let parent = stack.last().copied();
                let table = table_for_lbrace(parse, catalog, tokens, index, parent);
                if root_table.is_none()
                    && matches!(parent, Some(BodyTarget::RootSelection))
                    && let BodyTarget::Table(table) = table
                {
                    root_table = Some(table);
                }
                stack.push(table);
            }
            Some(SyntaxToken::RBrace) => {
                stack.pop();
            }
            _ => {}
        }
    }
    root_table
}

fn matching_lpar_before(tokens: &[&SyntaxNode], rpar_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens[..rpar_index].iter().enumerate().rev() {
        match token_kind(token) {
            Some(SyntaxToken::RPar) => depth += 1,
            Some(SyntaxToken::LPar) if depth == 0 => return Some(index),
            Some(SyntaxToken::LPar) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn unmatched_lpar_index(tokens: &[&SyntaxNode]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().rev() {
        match token_kind(token) {
            Some(SyntaxToken::RPar) => depth += 1,
            Some(SyntaxToken::LPar) if depth == 0 => return Some(index),
            Some(SyntaxToken::LPar) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn table_ref_before_lpar(
    parse: &ParseResult,
    tokens: &[&SyntaxNode],
    lpar_index: usize,
) -> Option<String> {
    relation_ref_ending_at(parse, tokens, lpar_index.checked_sub(1)?)
}

fn relation_ref_ending_at(
    parse: &ParseResult,
    tokens: &[&SyntaxNode],
    end_index: usize,
) -> Option<String> {
    let mut parts = Vec::new();
    let mut index = end_index;
    let mut expected_end = tokens[end_index].range.end;
    while let Some(SyntaxToken::Name | SyntaxToken::Dot | SyntaxToken::ColonColon) =
        token_kind(tokens[index])
    {
        if tokens[index].range.end != expected_end {
            break;
        }
        parts.push(parse.source.text(tokens[index].range).to_string());
        expected_end = tokens[index].range.start;
        let Some(next) = index.checked_sub(1) else {
            break;
        };
        index = next;
    }
    parts.reverse();
    (!parts.is_empty()).then(|| parts.join(""))
}

fn used_clauses_in_tokens(tokens: &[&SyntaxNode]) -> UsedClauses {
    let mut used = UsedClauses::default();
    for token in tokens {
        match token_kind(token) {
            Some(SyntaxToken::Where) => used.where_clause = true,
            Some(SyntaxToken::Order) => used.order_by = true,
            Some(SyntaxToken::Limit) => used.limit = true,
            Some(SyntaxToken::Offset) => used.offset = true,
            _ => {}
        }
    }
    used
}

fn token_kind(token: &SyntaxNode) -> Option<SyntaxToken> {
    match token.cst_kind {
        CstKind::Token(kind) => Some(kind),
        CstKind::Rule(_) => None,
    }
}

fn is_clause_keyword(kind: Option<SyntaxToken>) -> bool {
    matches!(
        kind,
        Some(SyntaxToken::Where | SyntaxToken::Order | SyntaxToken::Limit | SyntaxToken::Offset)
    )
}

fn is_operator_token(kind: Option<SyntaxToken>) -> bool {
    matches!(
        kind,
        Some(
            SyntaxToken::Eq
                | SyntaxToken::Ne
                | SyntaxToken::Gt
                | SyntaxToken::Ge
                | SyntaxToken::Lt
                | SyntaxToken::Le
                | SyntaxToken::Like
        )
    )
}

fn is_predicate_value_token(kind: Option<SyntaxToken>) -> bool {
    matches!(
        kind,
        Some(
            SyntaxToken::Name
                | SyntaxToken::String
                | SyntaxToken::Number
                | SyntaxToken::True
                | SyntaxToken::False
                | SyntaxToken::Null
        )
    )
}
