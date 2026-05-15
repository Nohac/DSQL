use crate::DocumentSnapshot;
use dsql_core::{
    Catalog, Clause, Definition, Expr, FieldCheckResult, ParseResult, Selection, SelectionKind,
    SourceSnapshot, TableId, TextRange,
};

#[derive(Clone, Debug)]
pub struct DocumentSemanticTokens {
    pub snapshot: DocumentSnapshot,
    pub tokens: Vec<SemanticTokenInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticTokenInfo {
    pub range: TextRange,
    pub kind: SemanticTokenKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticTokenKind {
    Schema,
    Table,
    Relation,
    Column,
    Fragment,
    Alias,
}

pub(crate) fn semantic_tokens_at(parse: &ParseResult, catalog: &Catalog) -> Vec<SemanticTokenInfo> {
    let mut tokens = Vec::new();
    for definition in parse.source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                if let Some(name) = &query.name {
                    tokens.push(SemanticTokenInfo {
                        range: name.range,
                        kind: SemanticTokenKind::Fragment,
                    });
                }
                for selection in &query.selections {
                    add_table_ref_tokens(&mut tokens, &parse.source, selection.name.range);
                    if let Some(table) = catalog.table_ref(&selection.name.text) {
                        add_clause_tokens(&mut tokens, catalog, table.id, selection);
                        add_selection_tokens(
                            &mut tokens,
                            &parse.source,
                            catalog,
                            table.id,
                            &selection.selections,
                        );
                    }
                }
            }
            Definition::Fragment(fragment) => {
                if let Some(name) = &fragment.name {
                    tokens.push(SemanticTokenInfo {
                        range: name.range,
                        kind: SemanticTokenKind::Fragment,
                    });
                }
                if let Some(on) = &fragment.on {
                    add_table_ref_tokens(&mut tokens, &parse.source, on.range);
                    if let Some(table) = catalog.table_ref(&on.text) {
                        add_selection_tokens(
                            &mut tokens,
                            &parse.source,
                            catalog,
                            table.id,
                            &fragment.selections,
                        );
                    }
                }
            }
        }
    }
    tokens.sort_by_key(|token| (token.range.start, token.range.end));
    tokens
}

fn add_selection_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
) {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            tokens.push(SemanticTokenInfo {
                range: selection.name.range,
                kind: SemanticTokenKind::Fragment,
            });
            continue;
        }
        if let Some(alias) = &selection.alias {
            tokens.push(SemanticTokenInfo {
                range: alias.range,
                kind: SemanticTokenKind::Alias,
            });
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(_) => {
                tokens.push(SemanticTokenInfo {
                    range: selection.name.range,
                    kind: SemanticTokenKind::Column,
                });
            }
            FieldCheckResult::Relation(relation) => {
                add_relation_ref_tokens(tokens, source, selection.name.range);
                add_selection_tokens(
                    tokens,
                    source,
                    catalog,
                    relation.table.id,
                    &selection.selections,
                );
            }
            FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {}
        }
        add_clause_tokens(tokens, catalog, table, selection);
    }
}

fn add_clause_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    catalog: &Catalog,
    table: TableId,
    selection: &Selection,
) {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                add_expr_tokens(tokens, catalog, table, &where_clause.predicate);
            }
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    if matches!(
                        catalog.check_field(table, &item.field.text),
                        FieldCheckResult::Column(_)
                    ) {
                        tokens.push(SemanticTokenInfo {
                            range: item.field.range,
                            kind: SemanticTokenKind::Column,
                        });
                    }
                }
            }
            Clause::Limit(_) | Clause::Offset(_) => {}
        }
    }
}

fn add_expr_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    catalog: &Catalog,
    table: TableId,
    expr: &Expr,
) {
    match expr {
        Expr::Name(name) => {
            if matches!(
                catalog.check_field(table, &name.text),
                FieldCheckResult::Column(_)
            ) {
                tokens.push(SemanticTokenInfo {
                    range: name.range,
                    kind: SemanticTokenKind::Column,
                });
            }
        }
        Expr::Binary { left, right, .. } => {
            add_expr_tokens(tokens, catalog, table, left);
            add_expr_tokens(tokens, catalog, table, right);
        }
        Expr::Literal(_) => {}
    }
}

fn add_table_ref_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    range: TextRange,
) {
    add_qualified_ref_tokens(tokens, source, range, SemanticTokenKind::Table);
}

fn add_relation_ref_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    range: TextRange,
) {
    add_qualified_ref_tokens(tokens, source, range, SemanticTokenKind::Relation);
}

fn add_qualified_ref_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    range: TextRange,
    tail_kind: SemanticTokenKind,
) {
    let text = source.text(range);
    let relation_end = text.find("::").unwrap_or(text.len());
    let relation_range = TextRange {
        start: range.start,
        end: range.start + relation_end as u32,
    };
    let relation_text = &text[..relation_end];
    if let Some(dot) = relation_text.find('.') {
        tokens.push(SemanticTokenInfo {
            range: TextRange {
                start: relation_range.start,
                end: relation_range.start + dot as u32,
            },
            kind: SemanticTokenKind::Schema,
        });
        tokens.push(SemanticTokenInfo {
            range: TextRange {
                start: relation_range.start + dot as u32 + 1,
                end: relation_range.end,
            },
            kind: tail_kind,
        });
    } else {
        tokens.push(SemanticTokenInfo {
            range: relation_range,
            kind: tail_kind,
        });
    }
}
