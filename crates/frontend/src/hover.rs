use crate::range_contains;
use dsql_core::{
    Catalog, Clause, Definition, Expr, FieldCheckResult, Selection, SelectionKind, SourceFile,
    TableId, VariableBinding, VariableRole, VariableSource, infer_variable_bindings,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverInfo {
    pub label: String,
    pub detail: String,
    pub markdown: String,
}

pub(crate) fn hover_at(
    source_file: &SourceFile,
    catalog: &Catalog,
    byte: usize,
) -> Option<HoverInfo> {
    let variable_bindings = infer_variable_bindings(source_file, catalog).bindings;
    if let Some(binding) = variable_bindings
        .iter()
        .find(|binding| range_contains(binding.range, byte))
    {
        return Some(variable_hover(binding));
    }

    for definition in source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                if let Some(name) = query.name()
                    && range_contains(name.range, byte)
                {
                    return Some(query_hover(
                        &name.text,
                        variable_bindings.iter().filter(|binding| {
                            range_contains(query.range, binding.range.start as usize)
                        }),
                    ));
                }
                for selection in &query.selections {
                    if !range_contains(selection.name.range, byte)
                        && !range_contains(selection.range, byte)
                    {
                        continue;
                    }
                    let table = catalog.table_ref(&selection.name.text)?;
                    if range_contains(selection.name.range, byte) {
                        return Some(HoverInfo {
                            label: selection.name.text.clone(),
                            detail: format!("table {}.{}", table.schema, table.name),
                            markdown: table_hover_markdown(table),
                        });
                    }
                    if let Some(info) =
                        hover_in_selections(catalog, table.id, &selection.selections, byte)
                    {
                        return Some(info);
                    }
                    if let Some(info) = hover_in_clauses(catalog, table.id, selection, byte) {
                        return Some(info);
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
                let Some(table) = catalog.table_ref(&on.text) else {
                    continue;
                };
                if range_contains(on.range, byte) {
                    return Some(HoverInfo {
                        label: on.text.clone(),
                        detail: format!("table {}.{}", table.schema, table.name),
                        markdown: table_hover_markdown(table),
                    });
                }
                if let Some(info) =
                    hover_in_selections(catalog, table.id, &fragment.selections, byte)
                {
                    return Some(info);
                }
            }
        }
    }
    None
}

fn query_hover<'a>(name: &str, bindings: impl Iterator<Item = &'a VariableBinding>) -> HoverInfo {
    let shape = variable_shape_markdown(bindings);
    HoverInfo {
        label: name.to_string(),
        detail: "query".to_string(),
        markdown: if shape.is_empty() {
            format!("### Query `{name}`\n\nNo variables.")
        } else {
            format!("### Query `{name}`\n\n#### Variables\n\n```yaml\n{shape}```")
        },
    }
}

fn hover_in_selections(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    byte: usize,
) -> Option<HoverInfo> {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        if !range_contains(selection.name.range, byte) && !range_contains(selection.range, byte) {
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) if range_contains(selection.name.range, byte) => {
                return Some(HoverInfo {
                    label: selection.name.text.clone(),
                    detail: format!("column: {}", column.data_type.as_str()),
                    markdown: column_hover_markdown(catalog, column),
                });
            }
            FieldCheckResult::Relation(related) => {
                if range_contains(selection.name.range, byte) {
                    return Some(HoverInfo {
                        label: selection.name.text.clone(),
                        detail: format!(
                            "relation: {}.{}",
                            related.table.schema, related.table.name
                        ),
                        markdown: relation_hover_markdown(catalog, &related),
                    });
                }
                if let Some(info) =
                    hover_in_selections(catalog, related.table.id, &selection.selections, byte)
                {
                    return Some(info);
                }
            }
            FieldCheckResult::NotFound
            | FieldCheckResult::Column(_)
            | FieldCheckResult::AmbiguousRelation { .. } => {}
        }
        if let Some(info) = hover_in_clauses(catalog, table, selection, byte) {
            return Some(info);
        }
    }
    None
}

fn hover_in_clauses(
    catalog: &Catalog,
    table: TableId,
    selection: &Selection,
    byte: usize,
) -> Option<HoverInfo> {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                if let Some(info) = hover_in_expr(catalog, table, &where_clause.predicate, byte) {
                    return Some(info);
                }
            }
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    if range_contains(item.field.range, byte)
                        && let FieldCheckResult::Column(column) =
                            catalog.check_field(table, &item.field.text)
                    {
                        return Some(HoverInfo {
                            label: item.field.text.clone(),
                            detail: format!("column: {}", column.data_type.as_str()),
                            markdown: column_hover_markdown(catalog, column),
                        });
                    }
                }
            }
            Clause::Limit(_) | Clause::Offset(_) => {}
        }
    }
    None
}

fn hover_in_expr(catalog: &Catalog, table: TableId, expr: &Expr, byte: usize) -> Option<HoverInfo> {
    match expr {
        Expr::Name(name) => {
            if range_contains(name.range, byte)
                && let FieldCheckResult::Column(column) = catalog.check_field(table, &name.text)
            {
                return Some(HoverInfo {
                    label: name.text.clone(),
                    detail: format!("column: {}", column.data_type.as_str()),
                    markdown: column_hover_markdown(catalog, column),
                });
            }
        }
        Expr::Path(path) => return hover_in_path(catalog, table, path, byte),
        Expr::Binary { left, right, .. } => {
            return hover_in_expr(catalog, table, left, byte)
                .or_else(|| hover_in_expr(catalog, table, right, byte));
        }
        Expr::Variable(_) => {}
        Expr::Literal(_) => {}
    }
    None
}

fn hover_in_path(
    catalog: &Catalog,
    table: TableId,
    path: &dsql_core::ScopedPath,
    byte: usize,
) -> Option<HoverInfo> {
    let mut current_table = table;
    for (index, segment) in path.segments.iter().enumerate() {
        if index + 1 == path.segments.len() {
            if range_contains(segment.range, byte)
                && let FieldCheckResult::Column(column) =
                    catalog.check_field(current_table, &segment.field_ref())
            {
                return Some(HoverInfo {
                    label: segment.field_ref(),
                    detail: format!("column: {}", column.data_type.as_str()),
                    markdown: column_hover_markdown(catalog, column),
                });
            }
            return None;
        }
        let FieldCheckResult::Relation(relation) =
            catalog.check_field(current_table, &segment.field_ref())
        else {
            return None;
        };
        if range_contains(segment.range, byte) {
            return Some(HoverInfo {
                label: segment.field_ref(),
                detail: format!(
                    "relation to {}.{}",
                    relation.table.schema, relation.table.name
                ),
                markdown: relation_hover_markdown(catalog, &relation),
            });
        }
        current_table = relation.table.id;
    }
    None
}

fn variable_hover(binding: &VariableBinding) -> HoverInfo {
    let label = variable_label(binding);
    HoverInfo {
        label,
        detail: format!("variable: {}", binding.data_type.as_str()),
        markdown: variable_hover_markdown(binding),
    }
}

fn variable_label(binding: &VariableBinding) -> String {
    let prefix = match binding.source {
        VariableSource::Structured => "$",
        VariableSource::TopLevel => "$$",
    };
    binding.name.as_ref().map_or_else(
        || prefix.to_string(),
        |name| {
            let mut label = prefix.to_string();
            label.push_str(name);
            label
        },
    )
}

fn variable_hover_markdown(binding: &VariableBinding) -> String {
    format!(
        "### Variable `{}`\n\n- Path: `{}`\n- Type: `{}`\n- Role: `{}`",
        variable_label(binding),
        binding.path,
        binding.data_type.as_str(),
        variable_role_label(binding.role)
    )
}

#[derive(Default)]
struct VariableShapeNode {
    children: BTreeMap<String, VariableShapeNode>,
    value: Option<String>,
}

fn variable_shape_markdown<'a>(bindings: impl Iterator<Item = &'a VariableBinding>) -> String {
    let mut root = VariableShapeNode::default();
    for binding in bindings {
        insert_variable_shape(
            &mut root,
            &binding.path.split('.').collect::<Vec<_>>(),
            binding.data_type.as_str(),
        );
    }
    let mut out = String::new();
    render_variable_shape(&root, 0, &mut out);
    out
}

fn insert_variable_shape(node: &mut VariableShapeNode, path: &[&str], data_type: &str) {
    let Some((head, tail)) = path.split_first() else {
        node.value = Some(data_type.to_string());
        return;
    };
    insert_variable_shape(
        node.children.entry((*head).to_string()).or_default(),
        tail,
        data_type,
    );
}

fn render_variable_shape(node: &VariableShapeNode, indent: usize, out: &mut String) {
    for (key, child) in &node.children {
        out.push_str(&"  ".repeat(indent));
        out.push_str(key);
        if child.children.is_empty() {
            out.push_str(": ");
            out.push_str(child.value.as_deref().unwrap_or("unknown"));
            out.push('\n');
        } else {
            out.push_str(":\n");
            render_variable_shape(child, indent + 1, out);
        }
    }
}

fn variable_role_label(role: VariableRole) -> &'static str {
    match role {
        VariableRole::WhereValue => "where value",
        VariableRole::Limit => "limit",
        VariableRole::Offset => "offset",
    }
}

fn table_hover_markdown(table: &dsql_core::Table) -> String {
    format!(
        "### Table `{}`\n\n- Schema: `{}`\n- Columns: {}\n- Primary key columns: {}\n- Outgoing foreign keys: {}\n- Incoming foreign keys: {}",
        table.name,
        table.schema,
        table.columns.len(),
        table.primary_key.len(),
        table.foreign_keys_from.len(),
        table.foreign_keys_to.len()
    )
}

fn column_hover_markdown(catalog: &Catalog, column: &dsql_core::Column) -> String {
    let table = catalog.table_by_id(column.table);
    let primary_key = table.is_some_and(|table| table.primary_key.contains(&column.id));
    let table_name = table.map_or("<unknown>", |table| table.name.as_str());
    let schema_name = table.map_or("<unknown>", |table| table.schema.as_str());
    format!(
        "### Column `{}`\n\n- Table: `{}.{}`\n- Type: `{}`\n- Nullable: {}\n- Primary key: {}\n- Unique: {}\n- Indexed: {}",
        column.name,
        schema_name,
        table_name,
        column.data_type.as_str(),
        yes_no(!column.not_null),
        yes_no(primary_key),
        yes_no(column.is_unique),
        yes_no(column.is_indexed)
    )
}

fn relation_hover_markdown(catalog: &Catalog, relation: &dsql_core::RelationField<'_>) -> String {
    let from_table = catalog.table_by_id(relation.foreign_key.from_table);
    let to_table = catalog.table_by_id(relation.foreign_key.to_table);
    let from_columns = relation
        .foreign_key
        .from_columns
        .iter()
        .filter_map(|id| catalog.column_by_id(*id))
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let to_columns = relation
        .foreign_key
        .to_columns
        .iter()
        .filter_map(|id| catalog.column_by_id(*id))
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let from_table_name = from_table.map_or("<unknown>", |table| table.name.as_str());
    let to_table_name = to_table.map_or("<unknown>", |table| table.name.as_str());
    format!(
        "### Relation `{}`\n\n- Target: `{}.{}`\n- Foreign key: `{}.{}` ({}) -> `{}.{}` ({})",
        relation.name,
        relation.table.schema,
        relation.table.name,
        from_table.map_or("<unknown>", |table| table.schema.as_str()),
        from_table_name,
        qualify_columns(from_table_name, &from_columns),
        to_table.map_or("<unknown>", |table| table.schema.as_str()),
        to_table_name,
        qualify_columns(to_table_name, &to_columns)
    )
}

fn qualify_columns(table: &str, columns: &str) -> String {
    columns
        .split(", ")
        .filter(|column| !column.is_empty())
        .map(|column| format!("{table}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
