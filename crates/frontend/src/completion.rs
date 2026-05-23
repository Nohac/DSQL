use crate::cursor::{CursorContext, UsedClauses, cursor_context};
use dsql_core::{BinaryOp, Catalog, DataType, FragmentRecord, TableId, Token};
use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
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
            insert_text: None,
        }
    }

    fn operator(op: BinaryOp) -> Self {
        Self {
            label: op
                .label()
                .expect("completion operator must have a label")
                .to_string(),
            kind: CompletionKind::Operator,
            detail: Some(op.detail().to_string()),
            insert_text: None,
        }
    }

    fn token(token: Token, kind: CompletionKind, detail: &'static str) -> Self {
        Self {
            label: token
                .completion_label()
                .expect("completion token must have a label")
                .to_string(),
            kind,
            detail: Some(detail.to_string()),
            insert_text: None,
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

#[derive(Clone, Debug, Default, PartialEq, Facet)]
pub(crate) struct CompletionScope {
    pub fragments: Vec<FragmentRecord>,
}

pub(crate) fn completions_at(
    parse: &dsql_core::ParseResult,
    catalog: &Catalog,
    byte: usize,
    scope: &CompletionScope,
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
            fragment_completions(parse, catalog, table, byte, scope)
        }
        CursorContext::SelectionBody { table } => field_completions(catalog, table),
        CursorContext::ClauseList { table: _, used } => clause_keyword_completions(used),
        CursorContext::WhereBooleanOperator { table: _, used } => {
            where_boolean_operator_completions(used)
        }
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

#[cfg(test)]
pub(crate) fn completions_at_empty_scope(
    parse: &dsql_core::ParseResult,
    catalog: &Catalog,
    byte: usize,
) -> Vec<CompletionItem> {
    completions_at(parse, catalog, byte, &CompletionScope::default())
}

fn fragment_completions(
    parse: &dsql_core::ParseResult,
    catalog: &Catalog,
    table: TableId,
    byte: usize,
    scope: &CompletionScope,
) -> Vec<CompletionItem> {
    let mut completions = parse
        .source_file
        .fragments()
        .filter_map(|fragment| {
            let name = fragment.name.as_ref()?;
            let on = fragment.on.as_ref()?;
            fragment_completion(parse, catalog, table, byte, &name.text, &on.text)
        })
        .collect::<Vec<_>>();
    completions.extend(scope.fragments.iter().filter_map(|fragment| {
        let on = fragment.on.as_ref()?;
        fragment_completion(parse, catalog, table, byte, &fragment.key.name, on)
    }));
    completions.sort_by(|left, right| left.label.cmp(&right.label));
    completions.dedup_by(|left, right| left.label == right.label);
    completions
}

fn fragment_completion(
    parse: &dsql_core::ParseResult,
    catalog: &Catalog,
    table: TableId,
    byte: usize,
    name: &str,
    on: &str,
) -> Option<CompletionItem> {
    let target = catalog.table_ref(on)?;
    (target.id == table).then(|| CompletionItem {
        label: name.to_string(),
        kind: CompletionKind::Fragment,
        detail: Some(format!("fragment on {on}")),
        insert_text: Some(fragment_insert_text(parse, byte, name)),
    })
}

fn fragment_insert_text(parse: &dsql_core::ParseResult, byte: usize, name: &str) -> String {
    let source = parse.source.to_arc_str();
    let source = source.as_bytes();
    let mut name_start = byte;
    while name_start > 0 && is_identifier_byte(source[name_start - 1]) {
        name_start -= 1;
    }
    if name_start < byte && source.get(name_start.saturating_sub(3)..name_start) == Some(b"...") {
        let typed = std::str::from_utf8(&source[name_start..byte]).unwrap_or_default();
        return name.strip_prefix(typed).unwrap_or(name).to_string();
    }

    let mut dot_count = 0usize;
    let mut index = byte;
    while index > 0 && source[index - 1] == b'.' && dot_count < 3 {
        dot_count += 1;
        index -= 1;
    }
    format!("{}{}", ".".repeat(3usize.saturating_sub(dot_count)), name)
}

fn is_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
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
            insert_text: None,
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
            insert_text: None,
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
            insert_text: None,
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
            insert_text: None,
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

fn where_boolean_operator_completions(used: UsedClauses) -> Vec<CompletionItem> {
    let mut completions = keyword_completions(&[
        CompletionAtom::Token(Token::And, "combine predicates"),
        CompletionAtom::Token(Token::Or, "match either predicate"),
    ]);
    completions.extend(clause_keyword_completions(used));
    completions
}

fn predicate_scope_completions() -> Vec<CompletionItem> {
    vec![
        CompletionItem {
            label: ".".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("current scope".to_string()),
            insert_text: None,
        },
        CompletionItem {
            label: "~".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("root scope".to_string()),
            insert_text: None,
        },
        CompletionItem {
            label: "..".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("parent scope".to_string()),
            insert_text: None,
        },
    ]
}

fn operator_completions(data_type: DataType) -> Vec<CompletionItem> {
    data_type
        .operator_ops()
        .iter()
        .copied()
        .map(CompletionItem::operator)
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
