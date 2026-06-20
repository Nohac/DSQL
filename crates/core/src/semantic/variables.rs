use crate::{
    BinaryOp, Catalog, DataType, FieldCheckResult, TableId, TableResolution,
    definition::{
        DefinitionResolver, FragmentMap, FragmentRecord, QueryRecord, extract_definitions,
    },
    syntax::{
        BinaryOperator, Clause, Expr, OperatorVariable, OrderByItem, PathScope, ScopedPath,
        Selection, SelectionKind, SortDirectionExpr, SourceFile, TextRange, ValueVariable,
        VariableScope,
    },
    variable_path::{
        InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope, variable_path,
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

impl From<VariableScope> for VariableSource {
    fn from(scope: VariableScope) -> Self {
        match scope {
            VariableScope::Structured => Self::Structured,
            VariableScope::TopLevel => Self::TopLevel,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet, strum::AsRefStr)]
#[repr(u8)]
pub enum VariableRole {
    #[strum(serialize = "wherevalue")]
    WhereValue,
    #[strum(serialize = "comparisonoperator")]
    ComparisonOperator,
    #[strum(serialize = "sortdirection")]
    SortDirection,
    #[strum(serialize = "limit")]
    Limit,
    #[strum(serialize = "offset")]
    Offset,
}

pub fn infer_variable_bindings(source_file: &SourceFile, catalog: &Catalog) -> VariableBindings {
    let extracted = extract_definitions(source_file);
    let fragments = FragmentMap::from_file(&extracted);
    let mut bindings = Vec::new();
    for definition in extracted.definitions {
        match definition {
            crate::DefinitionRecord::Query(query) => {
                bindings.extend(infer_direct_query_variable_bindings(&query, catalog).bindings)
            }
            crate::DefinitionRecord::Fragment(fragment) => bindings
                .extend(infer_fragment_variable_bindings(&fragment, &fragments, catalog).bindings),
        }
    }
    bindings.sort_by_key(|binding| (binding.range.start, binding.range.end));
    VariableBindings { bindings }
}

pub fn infer_query_variable_bindings(
    query: &QueryRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> VariableBindings {
    let mut bindings = Vec::new();
    for selection in &query.selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        let TableResolution::Found(table) = catalog.resolve_table_ref_for(&selection.name.target)
        else {
            continue;
        };
        let path = vec![response_key(selection)];
        let scope = VariablePathScope::operation();
        collect_selection_bindings(
            catalog,
            resolver,
            table.id,
            table.id,
            selection,
            SelectionPath::body(path),
            &scope,
            &mut Vec::new(),
            &mut bindings,
        );
    }
    bindings.sort_by_key(|binding| (binding.range.start, binding.range.end));
    VariableBindings { bindings }
}

pub fn infer_fragment_variable_bindings(
    fragment: &FragmentRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> VariableBindings {
    let mut bindings = Vec::new();
    let Some(on) = fragment.on.as_ref() else {
        return VariableBindings { bindings };
    };
    let TableResolution::Found(table) = catalog.resolve_table_ref_for(on) else {
        return VariableBindings { bindings };
    };
    let scope = VariablePathScope::fragment();
    collect_selection_set_bindings(
        catalog,
        resolver,
        table.id,
        table.id,
        &fragment.selections,
        SelectionPath::fragment_root(),
        &scope,
        &mut Vec::new(),
        &mut bindings,
    );
    bindings.sort_by_key(|binding| (binding.range.start, binding.range.end));
    VariableBindings { bindings }
}

fn infer_direct_query_variable_bindings(
    query: &QueryRecord,
    catalog: &Catalog,
) -> VariableBindings {
    let mut bindings = Vec::new();
    for selection in &query.selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        let TableResolution::Found(table) = catalog.resolve_table_ref_for(&selection.name.target)
        else {
            continue;
        };
        collect_selection_bindings_without_fragments(
            catalog,
            table.id,
            table.id,
            selection,
            SelectionPath::body(vec![response_key(selection)]),
            &VariablePathScope::operation(),
            &mut bindings,
        );
    }
    VariableBindings { bindings }
}

#[allow(clippy::too_many_arguments)]
fn collect_selection_bindings(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    root_table: TableId,
    table: TableId,
    selection: &Selection,
    path: SelectionPath,
    scope: &VariablePathScope,
    visiting: &mut Vec<String>,
    bindings: &mut Vec<VariableBinding>,
) {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                collect_where_bindings(
                    catalog,
                    root_table,
                    table,
                    &path.parts,
                    scope,
                    &where_clause.predicate,
                    bindings,
                );
            }
            Clause::Limit(limit) => push_clause_variable(
                &path.parts,
                scope,
                VariableRole::Limit,
                DataType::Int,
                InputPathSegment::Limit,
                &limit.value,
                bindings,
            ),
            Clause::Offset(offset) => push_clause_variable(
                &path.parts,
                scope,
                VariableRole::Offset,
                DataType::Int,
                InputPathSegment::Offset,
                &offset.value,
                bindings,
            ),
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    collect_order_by_binding(catalog, table, &path.parts, scope, item, bindings);
                }
            }
        }
    }

    collect_selection_set_bindings(
        catalog,
        resolver,
        root_table,
        table,
        &selection.selections,
        path,
        scope,
        visiting,
        bindings,
    );
}

#[allow(clippy::too_many_arguments)]
fn collect_selection_set_bindings(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    root_table: TableId,
    table: TableId,
    selections: &[Selection],
    path: SelectionPath,
    scope: &VariablePathScope,
    visiting: &mut Vec<String>,
    bindings: &mut Vec<VariableBinding>,
) {
    for child in selections {
        if child.kind == SelectionKind::FragmentSpread {
            let Some(fragment) = resolver.fragment(&child.name.target.name.text) else {
                continue;
            };
            if visiting.iter().any(|name| name == &fragment.key.name) {
                continue;
            }
            visiting.push(fragment.key.name.clone());
            let spread_scope = scope.for_fragment_spread(&path, &fragment.key.name);
            collect_selection_set_bindings(
                catalog,
                resolver,
                root_table,
                table,
                &fragment.selections,
                SelectionPath::fragment_root(),
                &spread_scope,
                visiting,
                bindings,
            );
            visiting.pop();
            continue;
        }
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(table, &child.name)
        else {
            continue;
        };
        let child_path = path.relation_child_path(
            child
                .alias
                .as_ref()
                .map_or_else(|| relation.name.to_string(), |alias| alias.text.clone()),
        );
        collect_selection_bindings(
            catalog,
            resolver,
            root_table,
            relation.table.id,
            child,
            SelectionPath::body(child_path),
            scope,
            visiting,
            bindings,
        );
    }
}

fn collect_selection_bindings_without_fragments(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection: &Selection,
    path: SelectionPath,
    scope: &VariablePathScope,
    bindings: &mut Vec<VariableBinding>,
) {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                collect_where_bindings(
                    catalog,
                    root_table,
                    table,
                    &path.parts,
                    scope,
                    &where_clause.predicate,
                    bindings,
                );
            }
            Clause::Limit(limit) => push_clause_variable(
                &path.parts,
                scope,
                VariableRole::Limit,
                DataType::Int,
                InputPathSegment::Limit,
                &limit.value,
                bindings,
            ),
            Clause::Offset(offset) => push_clause_variable(
                &path.parts,
                scope,
                VariableRole::Offset,
                DataType::Int,
                InputPathSegment::Offset,
                &offset.value,
                bindings,
            ),
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    collect_order_by_binding(catalog, table, &path.parts, scope, item, bindings);
                }
            }
        }
    }

    for child in &selection.selections {
        if child.kind == SelectionKind::FragmentSpread {
            continue;
        }
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(table, &child.name)
        else {
            continue;
        };
        let child_path = path.relation_child_path(
            child
                .alias
                .as_ref()
                .map_or_else(|| relation.name.to_string(), |alias| alias.text.clone()),
        );
        collect_selection_bindings_without_fragments(
            catalog,
            root_table,
            relation.table.id,
            child,
            SelectionPath::body(child_path),
            scope,
            bindings,
        );
    }
}

fn collect_order_by_binding(
    catalog: &Catalog,
    table: TableId,
    selection_path: &[String],
    scope: &VariablePathScope,
    item: &OrderByItem,
    bindings: &mut Vec<VariableBinding>,
) {
    let variable = match &item.direction {
        SortDirectionExpr::Variable(variable) => variable,
        SortDirectionExpr::Static(_) => return,
    };
    let FieldCheckResult::Column(column) =
        catalog.check_field_ref(table, &qualified_name_relation_ref(item.field.clone()))
    else {
        return;
    };
    let inferred_path = [
        column.name.clone(),
        InputPathSegment::Direction.as_ref().to_string(),
    ];
    push_variable_binding(
        selection_path,
        VariableBindingContext {
            role: VariableRole::SortDirection,
            data_type: DataType::Unknown,
            scope,
            inferred_path: &inferred_path,
            anonymous_key: None,
            operators: Vec::new(),
            enum_values: crate::SortDirection::ALL
                .iter()
                .map(|direction| direction.label().to_string())
                .collect(),
        },
        variable,
        bindings,
    );
}

fn collect_where_bindings(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection_path: &[String],
    scope: &VariablePathScope,
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
                let anonymous_key =
                    if variable.name.is_none() && matches!(op, BinaryOperator::Variable(_)) {
                        Some(InputPathSegment::Value.as_ref())
                    } else {
                        None
                    };
                push_variable_binding(
                    selection_path,
                    VariableBindingContext {
                        role: VariableRole::WhereValue,
                        data_type,
                        scope,
                        inferred_path: &field_path,
                        anonymous_key,
                        operators: Vec::new(),
                        enum_values: Vec::new(),
                    },
                    variable,
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
        push_operator_binding(
            selection_path,
            scope,
            data_type,
            &field_path,
            operator,
            bindings,
        );
    }

    collect_where_bindings(
        catalog,
        root_table,
        table,
        selection_path,
        scope,
        left,
        bindings,
    );
    collect_where_bindings(
        catalog,
        root_table,
        table,
        selection_path,
        scope,
        right,
        bindings,
    );
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
    scope: &VariablePathScope,
    role: VariableRole,
    data_type: DataType,
    inferred_key: InputPathSegment,
    expr: &Expr,
    bindings: &mut Vec<VariableBinding>,
) {
    let Expr::Variable(variable) = expr else {
        return;
    };
    push_variable_binding(
        selection_path,
        VariableBindingContext {
            role,
            data_type,
            scope,
            inferred_path: &[inferred_key.as_ref().to_string()],
            anonymous_key: None,
            operators: Vec::new(),
            enum_values: Vec::new(),
        },
        variable,
        bindings,
    );
}

fn push_operator_binding(
    selection_path: &[String],
    scope: &VariablePathScope,
    data_type: DataType,
    inferred_path: &[String],
    variable: &OperatorVariable,
    bindings: &mut Vec<VariableBinding>,
) {
    let name = variable.name.as_ref().map(|name| name.text.clone());
    let key = name
        .as_ref()
        .cloned()
        .unwrap_or_else(|| InputPathSegment::Op.as_ref().to_string());
    let source = variable.scope.into();
    let path = variable_path(
        selection_path,
        VariablePathContext {
            role: VariableRole::ComparisonOperator,
            inferred_path,
            anonymous_key: None,
        },
        scope,
        variable.scope,
        Some(&key),
    );
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
            .filter_map(|operator| operator.dsql_label().map(str::to_string))
            .collect(),
    });
}

struct VariableBindingContext<'a> {
    role: VariableRole,
    data_type: DataType,
    scope: &'a VariablePathScope,
    operators: Vec<BinaryOp>,
    enum_values: Vec<String>,
    inferred_path: &'a [String],
    anonymous_key: Option<&'a str>,
}

fn push_variable_binding(
    selection_path: &[String],
    context: VariableBindingContext<'_>,
    variable: &ValueVariable,
    bindings: &mut Vec<VariableBinding>,
) {
    let name = variable.name.as_ref().map(|name| name.text.clone());
    let source = variable.scope.into();
    let path = variable_path(
        selection_path,
        VariablePathContext {
            role: context.role,
            inferred_path: context.inferred_path,
            anonymous_key: context.anonymous_key,
        },
        context.scope,
        variable.scope,
        name.as_deref(),
    );
    bindings.push(VariableBinding {
        range: variable.range,
        path,
        source,
        name,
        data_type: context.data_type,
        role: context.role,
        operators: context.operators,
        enum_values: context.enum_values,
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
        let field_ref = relation_ref.display_text();
        let FieldCheckResult::Relation(relation) =
            catalog.check_field_ref(current_table, &relation_ref.relation_ref())
        else {
            return None;
        };
        field_path.push(field_ref);
        current_table = relation.table.id;
    }
    let field_ref = last.display_text();
    let FieldCheckResult::Column(column) =
        catalog.check_field_ref(current_table, &last.relation_ref())
    else {
        return None;
    };
    field_path.push(field_ref);
    Some((column.data_type, field_path))
}

fn response_key(selection: &Selection) -> String {
    selection.alias.as_ref().map_or_else(
        || selection.name.output_name().to_string(),
        |alias| alias.text.clone(),
    )
}

fn qualified_name_relation_ref(target: crate::QualifiedNameRef) -> crate::RelationRef {
    crate::RelationRef {
        range: target.range,
        target,
        selector: None,
    }
}
