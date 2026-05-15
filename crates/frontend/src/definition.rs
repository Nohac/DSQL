use crate::range_contains;
use dsql_core::{
    Catalog, Clause, Definition, Expr, FieldCheckResult, Selection, SelectionKind, SourceFile,
    TableId, TextRange,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionResult {
    Source(SourceDefinition),
    Catalog(CatalogDefinition),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDefinition {
    pub uri: String,
    pub range: TextRange,
    pub kind: SourceDefinitionKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceDefinitionKind {
    Fragment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogDefinition {
    Table {
        schema: String,
        table: String,
    },
    Column {
        schema: String,
        table: String,
        column: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DefinitionTarget {
    Fragment { name: String },
    Catalog(CatalogDefinition),
}

pub(crate) fn definition_target_at(
    source_file: &SourceFile,
    catalog: &Catalog,
    byte: usize,
) -> Option<DefinitionTarget> {
    for definition in source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if !range_contains(selection.name.range, byte)
                        && !range_contains(selection.range, byte)
                    {
                        continue;
                    }
                    let table = catalog.table_ref(&selection.name.text)?;
                    if range_contains(selection.name.range, byte) {
                        return Some(table_target(table));
                    }
                    if let Some(target) =
                        definition_in_selections(catalog, table.id, &selection.selections, byte)
                    {
                        return Some(target);
                    }
                    if let Some(target) = definition_in_clauses(catalog, table.id, selection, byte)
                    {
                        return Some(target);
                    }
                }
            }
            Definition::Fragment(fragment) => {
                if !range_contains(fragment.range, byte) {
                    continue;
                }
                let Some(on) = &fragment.on else {
                    continue;
                };
                let table = catalog.table_ref(&on.text)?;
                if range_contains(on.range, byte) {
                    return Some(table_target(table));
                }
                if let Some(target) =
                    definition_in_selections(catalog, table.id, &fragment.selections, byte)
                {
                    return Some(target);
                }
            }
        }
    }
    None
}

pub(crate) fn find_fragment_definition(source_file: &SourceFile, name: &str) -> Option<TextRange> {
    source_file
        .fragments()
        .filter_map(|fragment| fragment.name.as_ref())
        .find(|fragment_name| fragment_name.text == name)
        .map(|fragment_name| fragment_name.range)
}

fn definition_in_selections(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    byte: usize,
) -> Option<DefinitionTarget> {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            if range_contains(selection.name.range, byte) {
                return Some(DefinitionTarget::Fragment {
                    name: selection.name.text.clone(),
                });
            }
            continue;
        }
        if !range_contains(selection.name.range, byte) && !range_contains(selection.range, byte) {
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) if range_contains(selection.name.range, byte) => {
                return Some(DefinitionTarget::Catalog(CatalogDefinition::Column {
                    schema: column.key.schema.clone(),
                    table: column.key.table.clone(),
                    column: column.name.clone(),
                }));
            }
            FieldCheckResult::Relation(relation) => {
                if range_contains(selection.name.range, byte) {
                    return Some(table_target(relation.table));
                }
                if let Some(target) = definition_in_selections(
                    catalog,
                    relation.table.id,
                    &selection.selections,
                    byte,
                ) {
                    return Some(target);
                }
                if let Some(target) =
                    definition_in_clauses(catalog, relation.table.id, selection, byte)
                {
                    return Some(target);
                }
            }
            FieldCheckResult::NotFound
            | FieldCheckResult::Column(_)
            | FieldCheckResult::AmbiguousRelation { .. } => {}
        }
        if let Some(target) = definition_in_clauses(catalog, table, selection, byte) {
            return Some(target);
        }
    }
    None
}

fn definition_in_clauses(
    catalog: &Catalog,
    table: TableId,
    selection: &Selection,
    byte: usize,
) -> Option<DefinitionTarget> {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                if let Some(target) =
                    definition_in_expr(catalog, table, &where_clause.predicate, byte)
                {
                    return Some(target);
                }
            }
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    if range_contains(item.field.range, byte)
                        && let FieldCheckResult::Column(column) =
                            catalog.check_field(table, &item.field.text)
                    {
                        return Some(DefinitionTarget::Catalog(CatalogDefinition::Column {
                            schema: column.key.schema.clone(),
                            table: column.key.table.clone(),
                            column: column.name.clone(),
                        }));
                    }
                }
            }
            Clause::Limit(_) | Clause::Offset(_) => {}
        }
    }
    None
}

fn definition_in_expr(
    catalog: &Catalog,
    table: TableId,
    expr: &Expr,
    byte: usize,
) -> Option<DefinitionTarget> {
    match expr {
        Expr::Name(name) => {
            if range_contains(name.range, byte)
                && let FieldCheckResult::Column(column) = catalog.check_field(table, &name.text)
            {
                return Some(DefinitionTarget::Catalog(CatalogDefinition::Column {
                    schema: column.key.schema.clone(),
                    table: column.key.table.clone(),
                    column: column.name.clone(),
                }));
            }
            None
        }
        Expr::Path(path) => definition_in_path(catalog, table, path, byte),
        Expr::Binary { left, right, .. } => definition_in_expr(catalog, table, left, byte)
            .or_else(|| definition_in_expr(catalog, table, right, byte)),
        Expr::Literal(_) => None,
    }
}

fn definition_in_path(
    catalog: &Catalog,
    table: TableId,
    path: &dsql_core::ScopedPath,
    byte: usize,
) -> Option<DefinitionTarget> {
    let mut current_table = table;
    for (index, segment) in path.segments.iter().enumerate() {
        if index + 1 == path.segments.len() {
            if range_contains(segment.range, byte)
                && let FieldCheckResult::Column(column) =
                    catalog.check_field(current_table, &segment.text)
            {
                return Some(DefinitionTarget::Catalog(CatalogDefinition::Column {
                    schema: column.key.schema.clone(),
                    table: column.key.table.clone(),
                    column: column.name.clone(),
                }));
            }
            return None;
        }
        let FieldCheckResult::Relation(relation) =
            catalog.check_field(current_table, &segment.text)
        else {
            return None;
        };
        if range_contains(segment.range, byte) {
            return Some(table_target(relation.table));
        }
        current_table = relation.table.id;
    }
    None
}

fn table_target(table: &dsql_core::Table) -> DefinitionTarget {
    DefinitionTarget::Catalog(CatalogDefinition::Table {
        schema: table.schema.clone(),
        table: table.name.clone(),
    })
}
