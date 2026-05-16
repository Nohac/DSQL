use crate::{
    BinaryOp, Catalog, DataType, FieldCheckResult, TableId, TableResolution,
    syntax::{
        BinaryOperator, Clause, Definition, Expr, OperatorVariable, OrderByItem, PathScope,
        ScopedPath, Selection, SelectionKind, SortDirectionExpr, SourceFile, TextRange,
        ValueVariable, VariableScope,
    },
};
use facet::Facet;

#[derive(Clone, Debug, Default, PartialEq, Eq, Facet)]
pub struct VariableBindings {
    pub bindings: Vec<VariableBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct VariableBinding {
    pub range: TextRange,
    pub path: String,
    pub source: VariableSource,
    pub name: Option<String>,
    pub data_type: DataType,
    pub role: VariableRole,
    pub operators: Vec<BinaryOp>,
    pub enum_values: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum VariableSource {
    Structured,
    TopLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum VariableRole {
    WhereValue,
    ComparisonOperator,
    SortDirection,
    Limit,
    Offset,
}

pub fn infer_variable_bindings(source_file: &SourceFile, catalog: &Catalog) -> VariableBindings {
    let mut bindings = Vec::new();
    for definition in source_file.definitions() {
        let Definition::Query(query) = definition else {
            continue;
        };
        for selection in &query.selections {
            if selection.kind == SelectionKind::FragmentSpread {
                continue;
            }
            let TableResolution::Found(table) = catalog.resolve_table_ref(&selection.name.text)
            else {
                continue;
            };
            let path = vec![response_key(selection)];
            collect_selection_bindings(
                catalog,
                table.id,
                table.id,
                selection,
                &path,
                &mut bindings,
            );
        }
    }
    bindings.sort_by_key(|binding| (binding.range.start, binding.range.end));
    VariableBindings { bindings }
}

fn collect_selection_bindings(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection: &Selection,
    path: &[String],
    bindings: &mut Vec<VariableBinding>,
) {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                collect_where_bindings(
                    catalog,
                    root_table,
                    table,
                    path,
                    &where_clause.predicate,
                    bindings,
                );
            }
            Clause::Limit(limit) => push_clause_variable(
                path,
                VariableRole::Limit,
                DataType::Int,
                "limit",
                &limit.value,
                bindings,
            ),
            Clause::Offset(offset) => push_clause_variable(
                path,
                VariableRole::Offset,
                DataType::Int,
                "offset",
                &offset.value,
                bindings,
            ),
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    collect_order_by_binding(catalog, table, path, item, bindings);
                }
            }
        }
    }

    for child in &selection.selections {
        if child.kind == SelectionKind::FragmentSpread {
            continue;
        }
        let FieldCheckResult::Relation(relation) = catalog.check_field(table, &child.name.text)
        else {
            continue;
        };
        let mut child_path = path.to_vec();
        child_path.push("body".to_string());
        child_path.push(response_key(child));
        collect_selection_bindings(
            catalog,
            root_table,
            relation.table.id,
            child,
            &child_path,
            bindings,
        );
    }
}

fn collect_order_by_binding(
    catalog: &Catalog,
    table: TableId,
    selection_path: &[String],
    item: &OrderByItem,
    bindings: &mut Vec<VariableBinding>,
) {
    let variable = match &item.direction {
        SortDirectionExpr::Variable(variable) => variable,
        SortDirectionExpr::Static(_) => return,
    };
    let FieldCheckResult::Column(column) = catalog.check_field(table, &item.field.text) else {
        return;
    };
    let inferred_path = [column.name.clone(), "direction".to_string()];
    push_variable_binding(
        selection_path,
        VariableRole::SortDirection,
        DataType::Unknown,
        &inferred_path,
        variable,
        Vec::new(),
        crate::SortDirection::ALL
            .iter()
            .map(|direction| direction.label().to_string())
            .collect(),
        bindings,
    );
}

fn collect_where_bindings(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection_path: &[String],
    expr: &Expr,
    bindings: &mut Vec<VariableBinding>,
) {
    let Expr::Binary {
        left, op, right, ..
    } = expr
    else {
        return;
    };

    match (left.as_ref(), right.as_ref()) {
        (Expr::Path(path), Expr::Variable(variable))
        | (Expr::Variable(variable), Expr::Path(path)) => {
            if let Some((data_type, field_path)) =
                resolve_predicate_path(catalog, root_table, table, path)
            {
                push_variable_binding(
                    selection_path,
                    VariableRole::WhereValue,
                    data_type,
                    &field_path,
                    variable,
                    Vec::new(),
                    Vec::new(),
                    bindings,
                );
            }
        }
        _ => {}
    }

    if let BinaryOperator::Variable(operator) = op
        && let Some((data_type, field_path)) =
            binary_field_path(catalog, root_table, table, left, right)
    {
        push_operator_binding(selection_path, data_type, &field_path, operator, bindings);
    }

    collect_where_bindings(catalog, root_table, table, selection_path, left, bindings);
    collect_where_bindings(catalog, root_table, table, selection_path, right, bindings);
}

fn binary_field_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    left: &Expr,
    right: &Expr,
) -> Option<(DataType, Vec<String>)> {
    let path = match (left, right) {
        (Expr::Path(path), _) | (_, Expr::Path(path)) => path,
        _ => return None,
    };
    resolve_predicate_path(catalog, root_table, table, path)
}

fn push_clause_variable(
    selection_path: &[String],
    role: VariableRole,
    data_type: DataType,
    inferred_key: &str,
    expr: &Expr,
    bindings: &mut Vec<VariableBinding>,
) {
    let Expr::Variable(variable) = expr else {
        return;
    };
    push_variable_binding(
        selection_path,
        role,
        data_type,
        &[inferred_key.to_string()],
        variable,
        Vec::new(),
        Vec::new(),
        bindings,
    );
}

fn push_operator_binding(
    selection_path: &[String],
    data_type: DataType,
    inferred_path: &[String],
    variable: &OperatorVariable,
    bindings: &mut Vec<VariableBinding>,
) {
    let name = variable.name.as_ref().map(|name| name.text.clone());
    let key = name
        .as_ref()
        .cloned()
        .unwrap_or_else(|| format!("{}_op", inferred_path.last().cloned().unwrap_or_default()));
    let source = match variable.scope {
        VariableScope::Structured => VariableSource::Structured,
        VariableScope::TopLevel => VariableSource::TopLevel,
    };
    let path = match variable.scope {
        VariableScope::Structured => {
            let mut parts = vec!["input".to_string()];
            parts.extend(selection_path.iter().cloned());
            parts.push("clause".to_string());
            parts.push("where".to_string());
            parts.extend(inferred_path.iter().cloned());
            parts.push(key);
            parts.join(".")
        }
        VariableScope::TopLevel => format!("params.{key}"),
    };
    bindings.push(VariableBinding {
        range: variable.range,
        path,
        source,
        name,
        data_type,
        role: VariableRole::ComparisonOperator,
        operators: variable.allowed.clone(),
        enum_values: variable
            .allowed
            .iter()
            .filter_map(|operator| operator.label().map(str::to_string))
            .collect(),
    });
}

fn push_variable_binding(
    selection_path: &[String],
    role: VariableRole,
    data_type: DataType,
    inferred_path: &[String],
    variable: &ValueVariable,
    operators: Vec<BinaryOp>,
    enum_values: Vec<String>,
    bindings: &mut Vec<VariableBinding>,
) {
    let name = variable.name.as_ref().map(|name| name.text.clone());
    let key = name.as_ref().map_or_else(
        || inferred_path.last().cloned().unwrap_or_default(),
        Clone::clone,
    );
    let source = match variable.scope {
        VariableScope::Structured => VariableSource::Structured,
        VariableScope::TopLevel => VariableSource::TopLevel,
    };
    let path = match variable.scope {
        VariableScope::Structured => {
            let mut parts = vec!["input".to_string()];
            parts.extend(selection_path.iter().cloned());
            parts.push("clause".to_string());
            parts.push(
                match role {
                    VariableRole::WhereValue => "where",
                    VariableRole::ComparisonOperator => "where",
                    VariableRole::SortDirection => "order_by",
                    VariableRole::Limit => "limit",
                    VariableRole::Offset => "offset",
                }
                .to_string(),
            );
            if matches!(
                role,
                VariableRole::WhereValue
                    | VariableRole::ComparisonOperator
                    | VariableRole::SortDirection
            ) {
                parts.extend(inferred_path.iter().cloned());
            }
            if name.is_some() {
                parts.push(key);
            }
            parts.join(".")
        }
        VariableScope::TopLevel => format!("params.{key}"),
    };
    bindings.push(VariableBinding {
        range: variable.range,
        path,
        source,
        name,
        data_type,
        role,
        operators,
        enum_values,
    });
}

fn resolve_predicate_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    path: &ScopedPath,
) -> Option<(DataType, Vec<String>)> {
    let mut current_table = match path.scope {
        PathScope::Current => table,
        PathScope::Root => root_table,
        PathScope::Parent => return None,
    };
    let (last, relations) = path.segments.split_last()?;
    let mut field_path = Vec::new();
    for relation_ref in relations {
        let field_ref = relation_ref.field_ref();
        let FieldCheckResult::Relation(relation) = catalog.check_field(current_table, &field_ref)
        else {
            return None;
        };
        field_path.push(field_ref);
        current_table = relation.table.id;
    }
    let field_ref = last.field_ref();
    let FieldCheckResult::Column(column) = catalog.check_field(current_table, &field_ref) else {
        return None;
    };
    field_path.push(field_ref);
    Some((column.data_type, field_path))
}

fn response_key(selection: &Selection) -> String {
    selection.alias.as_ref().map_or_else(
        || unqualified_name(&selection.name.text).to_string(),
        |alias| alias.text.clone(),
    )
}

fn unqualified_name(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, name)| name)
}
