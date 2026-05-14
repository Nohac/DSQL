use crate::range_contains;
use dsql_core::{
    Catalog, CstKind, DataType, Definition, FieldCheckResult, ParseResult, Selection,
    SelectionKind, SyntaxNode, SyntaxToken, TableId, expected_tokens_at,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CursorContext {
    Root,
    SelectionBody { table: TableId },
    ClauseList { table: TableId, used: UsedClauses },
    WhereColumn { table: TableId },
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
    if let Some(context) = clause_context(parse, catalog, byte) {
        return context;
    }

    cst_selection_body_context(parse, catalog, byte)
        .or_else(|| selection_body_context(parse, catalog, byte))
        .unwrap_or(CursorContext::Root)
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
                        return Some(CursorContext::Root);
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
                    return Some(CursorContext::Root);
                };
                let Some(table) = catalog.table_ref(&on.text) else {
                    return Some(CursorContext::Root);
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
        | FieldCheckResult::AmbiguousRelation { .. } => Some(CursorContext::SelectionBody {
            table: parent_table,
        }),
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
    let mut stack = Vec::new();

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
        Some(Some(table)) => Some(CursorContext::SelectionBody { table }),
        Some(None) => Some(CursorContext::Root),
        None => None,
    }
}

fn table_for_lbrace(
    parse: &ParseResult,
    catalog: &Catalog,
    tokens: &[&SyntaxNode],
    lbrace_index: usize,
    parent_table: Option<Option<TableId>>,
) -> Option<TableId> {
    let selection_ref = selection_ref_before_lbrace(parse, tokens, lbrace_index)?;

    if selection_ref.is_query_body {
        return None;
    }

    if let Some(parent_table) = parent_table.flatten() {
        return Some(
            match catalog.check_field(parent_table, &selection_ref.name) {
                FieldCheckResult::Relation(relation) => relation.table.id,
                FieldCheckResult::Column(_)
                | FieldCheckResult::NotFound
                | FieldCheckResult::AmbiguousRelation { .. } => parent_table,
            },
        );
    }

    catalog.table_ref(&selection_ref.name).map(|table| table.id)
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
                name: parse.source.text(tokens[previous_index].range).to_string(),
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
    let expected = expected_tokens_at(&parse.source, byte);

    if let Some(context) =
        parsed_clause_context(parse, catalog, cursor.table, &cursor.suffix_tokens)
    {
        return Some(context);
    }

    if expected.contains(&SyntaxToken::Name)
        && let Some(context) = name_expected_context(&cursor)
    {
        return Some(context);
    }

    if cursor.suffix_tokens.is_empty()
        || expected.iter().any(|token| is_clause_keyword(Some(*token)))
        || expected.is_empty()
    {
        return Some(CursorContext::ClauseList {
            table: cursor.table,
            used: used_clauses_in_tokens(&cursor.suffix_tokens),
        });
    }

    None
}

struct ClauseCursor<'a> {
    table: TableId,
    suffix_tokens: Vec<&'a SyntaxNode>,
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
    let table = selection_clause_table(parse, catalog, byte, &table_ref)
        .or_else(|| catalog.table_ref(&table_ref).map(|table| table.id))?;

    Some(ClauseCursor {
        table,
        suffix_tokens,
    })
}

fn parsed_clause_context(
    parse: &ParseResult,
    catalog: &Catalog,
    table: TableId,
    suffix_tokens: &[&SyntaxNode],
) -> Option<CursorContext> {
    let clause_index = suffix_tokens
        .iter()
        .rposition(|token| is_clause_keyword(token_kind(token)))?;

    match token_kind(suffix_tokens[clause_index])? {
        SyntaxToken::Where => {
            let after_where = &suffix_tokens[clause_index + 1..];
            let Some(field) = after_where
                .iter()
                .find(|token| token_kind(token) == Some(SyntaxToken::Name))
            else {
                return Some(CursorContext::WhereColumn { table });
            };
            if after_where
                .iter()
                .any(|token| is_operator_token(token_kind(token)))
            {
                return None;
            }
            let field_name = parse.source.text(field.range);
            match catalog.check_field(table, field_name.as_ref()) {
                FieldCheckResult::Column(column) => Some(CursorContext::WhereOperator {
                    data_type: column.data_type,
                }),
                FieldCheckResult::Relation(_)
                | FieldCheckResult::NotFound
                | FieldCheckResult::AmbiguousRelation { .. } => None,
            }
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

fn name_expected_context(cursor: &ClauseCursor<'_>) -> Option<CursorContext> {
    let clause_index = cursor
        .suffix_tokens
        .iter()
        .rposition(|token| is_clause_keyword(token_kind(token)))?;

    match token_kind(cursor.suffix_tokens[clause_index])? {
        SyntaxToken::Where => Some(CursorContext::WhereColumn {
            table: cursor.table,
        }),
        SyntaxToken::Order
            if cursor
                .suffix_tokens
                .get(clause_index + 1)
                .and_then(|token| token_kind(token))
                == Some(SyntaxToken::By) =>
        {
            Some(CursorContext::OrderByColumn {
                table: cursor.table,
            })
        }
        _ => None,
    }
}

fn selection_clause_table(
    parse: &ParseResult,
    catalog: &Catalog,
    byte: usize,
    field: &str,
) -> Option<TableId> {
    for definition in parse.source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    let Some(table) = catalog.table_ref(&selection.name.text) else {
                        continue;
                    };
                    if let Some(table) =
                        nested_clause_table(catalog, table.id, selection, byte, field)
                    {
                        return Some(table);
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
                    if let Some(table) =
                        nested_clause_table(catalog, table.id, selection, byte, field)
                    {
                        return Some(table);
                    }
                }
            }
        }
    }
    None
}

fn nested_clause_table(
    catalog: &Catalog,
    parent_table: TableId,
    selection: &Selection,
    byte: usize,
    field: &str,
) -> Option<TableId> {
    if selection.kind == SelectionKind::FragmentSpread || !range_contains(selection.range, byte) {
        return None;
    }

    let current_table = match catalog.check_field(parent_table, &selection.name.text) {
        FieldCheckResult::Relation(relation) => relation.table.id,
        FieldCheckResult::Column(_)
        | FieldCheckResult::NotFound
        | FieldCheckResult::AmbiguousRelation { .. } => parent_table,
    };

    if selection.name.text == field && byte >= selection.name.range.end as usize {
        return Some(current_table);
    }

    selection
        .selections
        .iter()
        .find_map(|child| nested_clause_table(catalog, current_table, child, byte, field))
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
    let mut parts = Vec::new();
    let mut index = lpar_index.checked_sub(1)?;
    let mut expected_end = tokens[lpar_index].range.start;
    while let Some(SyntaxToken::Name | SyntaxToken::Dot) = token_kind(tokens[index]) {
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
        )
    )
}
