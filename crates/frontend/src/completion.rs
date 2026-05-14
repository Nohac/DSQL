use crate::range_contains;
use dsql_core::{Catalog, Definition, FieldCheckResult, Selection, SourceFile, TableId};

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
}

pub(crate) fn completions_at(
    source_file: &SourceFile,
    catalog: &Catalog,
    byte: usize,
) -> Vec<CompletionItem> {
    match selection_context(source_file, catalog, byte) {
        CompletionContext::Root => catalog
            .tables
            .iter()
            .map(|table| CompletionItem {
                label: completion_table_ref(&table.schema, &table.name),
                kind: CompletionKind::Table,
                detail: Some(format!("table {}.{}", table.schema, table.name)),
            })
            .collect(),
        CompletionContext::Table(table) => field_completions(catalog, table),
    }
}

enum CompletionContext {
    Root,
    Table(TableId),
}

fn selection_context(
    source_file: &SourceFile,
    catalog: &Catalog,
    byte: usize,
) -> CompletionContext {
    for definition in source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if !range_contains(selection.range, byte) {
                        continue;
                    }
                    let Some(table) = catalog.table_ref(&selection.name.text) else {
                        return CompletionContext::Root;
                    };
                    return nested_context(catalog, table.id, &selection.selections, byte)
                        .unwrap_or(CompletionContext::Table(table.id));
                }
            }
            Definition::Fragment(fragment) => {
                if !range_contains(fragment.range, byte) {
                    continue;
                }
                let Some(on) = &fragment.on else {
                    return CompletionContext::Root;
                };
                let Some(table) = catalog.table_ref(&on.text) else {
                    return CompletionContext::Root;
                };
                return nested_context(catalog, table.id, &fragment.selections, byte)
                    .unwrap_or(CompletionContext::Table(table.id));
            }
        }
    }
    CompletionContext::Root
}

fn nested_context(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    byte: usize,
) -> Option<CompletionContext> {
    for selection in selections {
        if !range_contains(selection.range, byte) {
            continue;
        }
        return match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Relation(related) => {
                nested_context(catalog, related.table.id, &selection.selections, byte)
                    .or(Some(CompletionContext::Table(related.table.id)))
            }
            FieldCheckResult::Column(_)
            | FieldCheckResult::NotFound
            | FieldCheckResult::AmbiguousRelation { .. } => Some(CompletionContext::Table(table)),
        };
    }
    None
}

fn field_completions(catalog: &Catalog, table: TableId) -> Vec<CompletionItem> {
    let mut completions = Vec::new();
    completions.extend(
        catalog
            .columns_for_table(table)
            .map(|column| CompletionItem {
                label: column.name.clone(),
                kind: CompletionKind::Column,
                detail: Some(column.data_type.as_str().to_string()),
            }),
    );
    completions.extend(
        catalog
            .relation_fields_for_table(table)
            .into_iter()
            .map(|relation| CompletionItem {
                label: completion_table_ref(&relation.table.schema, relation.name),
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

fn completion_table_ref(schema: &str, table: &str) -> String {
    if schema == Catalog::DEFAULT_SCHEMA {
        table.to_string()
    } else {
        format!("{schema}.{table}")
    }
}
