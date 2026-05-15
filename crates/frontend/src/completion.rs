use crate::cursor::{CursorContext, UsedClauses, cursor_context};
use dsql_core::{Catalog, DataType, Definition, SourceFile, TableId, Token};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

impl CompletionItem {
    fn keyword_token(token: Token, detail: &'static str) -> Self {
        Self::token(token, CompletionKind::Keyword, detail)
    }

    fn keyword_phrase(label: &'static str, detail: &'static str) -> Self {
        Self {
            label: label.to_string(),
            kind: CompletionKind::Keyword,
            detail: Some(detail.to_string()),
        }
    }

    fn operator_token(token: Token) -> Self {
        Self::token(token, CompletionKind::Operator, operator_detail(token))
    }

    fn token(token: Token, kind: CompletionKind, detail: &'static str) -> Self {
        Self {
            label: token
                .completion_label()
                .expect("completion token must have a label")
                .to_string(),
            kind,
            detail: Some(detail.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Table,
    Column,
    Relation,
    Fragment,
    Keyword,
    Operator,
}

pub(crate) fn completions_at(
    parse: &dsql_core::ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Vec<CompletionItem> {
    match cursor_context(parse, catalog, byte) {
        CursorContext::Invalid => Vec::new(),
        CursorContext::DocumentRoot => document_root_completions(),
        CursorContext::FragmentOnKeyword => {
            keyword_completions(&[CompletionAtom::Token(Token::On, "set fragment table")])
        }
        CursorContext::FragmentType => root_selection_completions(catalog),
        CursorContext::RootSelection => root_selection_completions(catalog),
        CursorContext::FragmentSpread { table } => {
            fragment_completions(catalog, &parse.source_file, table)
        }
        CursorContext::SelectionBody { table } => field_completions(catalog, table),
        CursorContext::ClauseList { table: _, used } => clause_keyword_completions(used),
        CursorContext::WhereScope => predicate_scope_completions(),
        CursorContext::WhereColumn { table } => field_completions(catalog, table),
        CursorContext::WhereRelationSelector { table, relation } => {
            relation_selector_completions(catalog, table, &relation)
        }
        CursorContext::OrderByColumn { table } => column_completions(catalog, table),
        CursorContext::WhereOperator { data_type } => operator_completions(data_type),
        CursorContext::SortDirection => keyword_completions(&[
            CompletionAtom::Token(Token::Asc, "sort ascending"),
            CompletionAtom::Token(Token::Desc, "sort descending"),
        ]),
    }
}

fn fragment_completions(
    catalog: &Catalog,
    source_file: &SourceFile,
    table: TableId,
) -> Vec<CompletionItem> {
    let mut completions = source_file
        .definitions()
        .filter_map(|definition| {
            let Definition::Fragment(fragment) = definition else {
                return None;
            };
            let name = fragment.name.as_ref()?;
            let on = fragment.on.as_ref()?;
            let target = catalog.table_ref(&on.text)?;
            (target.id == table).then(|| CompletionItem {
                label: name.text.clone(),
                kind: CompletionKind::Fragment,
                detail: Some(format!("fragment on {}", on.text)),
            })
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.label.cmp(&right.label));
    completions
}

fn document_root_completions() -> Vec<CompletionItem> {
    keyword_completions(&[
        CompletionAtom::Token(Token::Query, "define query"),
        CompletionAtom::Token(Token::Fragment, "define fragment"),
    ])
}

fn root_selection_completions(catalog: &Catalog) -> Vec<CompletionItem> {
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
    let relations = catalog.relation_fields_for_table(table);
    completions.extend(relations.iter().map(|relation| {
        let relation_count = relations
            .iter()
            .filter(|candidate| candidate.table.id == relation.table.id)
            .count();
        let table_ref = completion_table_ref(catalog, &relation.table.schema, relation.name);
        let label = if relation_count > 1 {
            format!("{table_ref}::{}", relation.selector)
        } else {
            table_ref
        };
        CompletionItem {
            label,
            kind: CompletionKind::Relation,
            detail: Some(format!(
                "relation to {}.{} via {}",
                relation.table.schema, relation.table.name, relation.selector
            )),
        }
    }));
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

fn relation_selector_completions(
    catalog: &Catalog,
    table: TableId,
    relation_name: &str,
) -> Vec<CompletionItem> {
    let mut completions = catalog
        .relation_fields_for_table(table)
        .into_iter()
        .filter(|relation| relation.name == relation_name)
        .map(|relation| CompletionItem {
            label: relation.selector,
            kind: CompletionKind::Relation,
            detail: Some(format!(
                "relation to {}.{}",
                relation.table.schema, relation.table.name
            )),
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.label.cmp(&right.label));
    completions.dedup_by(|left, right| left.label == right.label);
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

fn predicate_scope_completions() -> Vec<CompletionItem> {
    vec![
        CompletionItem {
            label: ".".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("current scope".to_string()),
        },
        CompletionItem {
            label: "~".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("root scope".to_string()),
        },
        CompletionItem {
            label: "..".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("parent scope".to_string()),
        },
    ]
}

fn operator_completions(data_type: DataType) -> Vec<CompletionItem> {
    data_type
        .operator_tokens()
        .iter()
        .copied()
        .map(CompletionItem::operator_token)
        .collect()
}

fn operator_detail(token: Token) -> &'static str {
    match token {
        Token::Eq => "equals",
        Token::Ne => "not equals",
        Token::Gt => "greater than",
        Token::Ge => "greater than or equal",
        Token::Lt => "less than",
        Token::Le => "less than or equal",
        Token::Like => "matches pattern",
        _ => "operator",
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CompletionAtom {
    Token(Token, &'static str),
    Phrase(&'static str, &'static str),
}

fn keyword_completions(items: &[CompletionAtom]) -> Vec<CompletionItem> {
    items
        .iter()
        .map(|item| match item {
            CompletionAtom::Token(token, detail) => CompletionItem::keyword_token(*token, detail),
            CompletionAtom::Phrase(label, detail) => CompletionItem::keyword_phrase(label, detail),
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
