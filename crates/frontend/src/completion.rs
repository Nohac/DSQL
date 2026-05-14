use crate::cursor::{CursorContext, UsedClauses, cursor_context};
use dsql_core::{Catalog, DataType, TableId, Token};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Table,
    Column,
    Relation,
    Keyword,
    Operator,
}

pub(crate) fn completions_at(
    parse: &dsql_core::ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Vec<CompletionItem> {
    match cursor_context(parse, catalog, byte) {
        CursorContext::Root => root_completions(catalog),
        CursorContext::SelectionBody { table } => field_completions(catalog, table),
        CursorContext::ClauseList { table: _, used } => clause_keyword_completions(used),
        CursorContext::WhereColumn { table } | CursorContext::OrderByColumn { table } => {
            column_completions(catalog, table)
        }
        CursorContext::WhereOperator { data_type } => operator_completions(data_type),
        CursorContext::SortDirection => keyword_completions(&[
            CompletionAtom::Token(Token::Asc, "sort ascending"),
            CompletionAtom::Token(Token::Desc, "sort descending"),
        ]),
    }
}

fn root_completions(catalog: &Catalog) -> Vec<CompletionItem> {
    catalog
        .tables
        .iter()
        .map(|table| CompletionItem {
            label: completion_table_ref(catalog, &table.schema, &table.name),
            kind: CompletionKind::Table,
            detail: Some(format!("table {}.{}", table.schema, table.name)),
        })
        .collect()
}

fn field_completions(catalog: &Catalog, table: TableId) -> Vec<CompletionItem> {
    let mut completions = Vec::new();
    completions.extend(column_completions(catalog, table));
    completions.extend(
        catalog
            .relation_fields_for_table(table)
            .into_iter()
            .map(|relation| CompletionItem {
                label: completion_table_ref(catalog, &relation.table.schema, relation.name),
                kind: CompletionKind::Relation,
                detail: Some(format!(
                    "relation to {}.{}",
                    relation.table.schema, relation.table.name
                )),
            }),
    );
    completions.sort_by(|left, right| left.label.cmp(&right.label));
    completions.dedup_by(|left, right| left.label == right.label);
    completions
}

fn column_completions(catalog: &Catalog, table: TableId) -> Vec<CompletionItem> {
    let mut completions = catalog
        .columns_for_table(table)
        .map(|column| CompletionItem {
            label: column.name.clone(),
            kind: CompletionKind::Column,
            detail: Some(column.data_type.as_str().to_string()),
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.label.cmp(&right.label));
    completions
}

fn clause_keyword_completions(used: UsedClauses) -> Vec<CompletionItem> {
    let mut keywords = Vec::new();
    if !used.where_clause {
        keywords.push(CompletionAtom::Token(Token::Where, "filter rows"));
    }
    if !used.order_by {
        keywords.push(CompletionAtom::Phrase("order by", "sort rows"));
    }
    if !used.limit {
        keywords.push(CompletionAtom::Token(Token::Limit, "limit rows"));
    }
    if !used.offset {
        keywords.push(CompletionAtom::Token(Token::Offset, "skip rows"));
    }
    keyword_completions(&keywords)
}

fn operator_completions(data_type: DataType) -> Vec<CompletionItem> {
    match data_type {
        DataType::Int | DataType::Timestamptz => operator_items(&[
            (Token::Eq, "equals"),
            (Token::Ne, "not equals"),
            (Token::Gt, "greater than"),
            (Token::Ge, "greater than or equal"),
            (Token::Lt, "less than"),
            (Token::Le, "less than or equal"),
        ]),
        DataType::Text
        | DataType::Uuid
        | DataType::Boolean
        | DataType::Json
        | DataType::Unknown => operator_items(&[(Token::Eq, "equals"), (Token::Ne, "not equals")]),
    }
}

fn operator_items(items: &[(Token, &str)]) -> Vec<CompletionItem> {
    items
        .iter()
        .map(|(token, detail)| CompletionItem {
            label: token
                .completion_label()
                .expect("operator completion tokens must have labels")
                .to_string(),
            kind: CompletionKind::Operator,
            detail: Some((*detail).to_string()),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CompletionAtom {
    Token(Token, &'static str),
    Phrase(&'static str, &'static str),
}

fn keyword_completions(items: &[CompletionAtom]) -> Vec<CompletionItem> {
    items
        .iter()
        .map(|item| {
            let (label, detail) = match item {
                CompletionAtom::Token(token, detail) => (
                    token
                        .completion_label()
                        .expect("keyword completion tokens must have labels"),
                    *detail,
                ),
                CompletionAtom::Phrase(label, detail) => (*label, *detail),
            };
            CompletionItem {
                label: label.to_string(),
                kind: CompletionKind::Keyword,
                detail: Some(detail.to_string()),
            }
        })
        .collect()
}

fn completion_table_ref(catalog: &Catalog, schema: &str, table: &str) -> String {
    if schema == catalog.default_schema() {
        table.to_string()
    } else {
        format!("{schema}.{table}")
    }
}
