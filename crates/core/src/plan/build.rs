use super::{
    FilterColumnScope, FilterExpr, FilterLiteral, NestedRelation, OrderByPlan, PlannedFile,
    Projection, QueryPlan, SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan,
};
use crate::{
    catalog::{Catalog, FieldCheckResult, ForeignKeyId, TableId, TableKey, TableResolution},
    definition::{DefinitionResolver, FragmentMap, QueryRecord, extract_definitions},
    syntax::{
        Definition, Diagnostic, DiagnosticCode, DiagnosticSource, Selection, SelectionKind,
        Severity, SourceFile,
    },
};

pub fn plan_file(source_file: &SourceFile) -> PlannedFile {
    plan_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn plan_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> PlannedFile {
    let extracted = extract_definitions(source_file);
    let resolver = FragmentMap::from_file(&extracted);
    let mut queries = Vec::new();
    let mut diagnostics = Vec::new();
    for definition in source_file.definitions() {
        let Definition::Query(query) = definition else {
            continue;
        };
        for selection in &query.selections {
            match catalog.resolve_table_ref(&selection.name.text) {
                TableResolution::Found(table) => {
                    let clauses = plan_clauses(catalog, table.id, table.id, selection);
                    if let Some(selections) = plan_selection_set(
                        catalog,
                        &resolver,
                        table.id,
                        table.id,
                        &clauses,
                        &selection.selections,
                        &mut diagnostics,
                    ) {
                        queries.push(QueryPlan {
                            root: table.id,
                            output_name: selection
                                .alias
                                .as_ref()
                                .map_or_else(|| table.name.clone(), |alias| alias.text.clone()),
                            clauses,
                            selections,
                        });
                    }
                }
                TableResolution::NotFound { reference } => diagnostics.push(planner_diagnostic(
                    selection.name.range,
                    DiagnosticCode::TableNotFound,
                    format!("table `{reference}` not found"),
                )),
                TableResolution::Ambiguous {
                    reference,
                    candidates,
                } => diagnostics.push(planner_diagnostic(
                    selection.name.range,
                    DiagnosticCode::AmbiguousTable,
                    format!(
                        "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                        reference,
                        format_table_candidates(&candidates)
                    ),
                )),
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    PlannedFile {
        queries,
        diagnostics,
    }
}

pub fn plan_query_definition(
    query: &QueryRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> PlannedFile {
    let mut queries = Vec::new();
    let mut diagnostics = Vec::new();
    for selection in &query.selections {
        if selection.kind == SelectionKind::FragmentSpread {
            diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::UnknownFragment,
                format!("fragment `{}` not found", selection.name.text),
            ));
            continue;
        }
        match catalog.resolve_table_ref(&selection.name.text) {
            TableResolution::Found(table) => {
                let clauses = plan_clauses(catalog, table.id, table.id, selection);
                if let Some(selections) = plan_selection_set(
                    catalog,
                    resolver,
                    table.id,
                    table.id,
                    &clauses,
                    &selection.selections,
                    &mut diagnostics,
                ) {
                    queries.push(QueryPlan {
                        root: table.id,
                        output_name: selection
                            .alias
                            .as_ref()
                            .map_or_else(|| table.name.clone(), |alias| alias.text.clone()),
                        clauses,
                        selections,
                    });
                }
            }
            TableResolution::NotFound { reference } => diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::TableNotFound,
                format!("table `{reference}` not found"),
            )),
            TableResolution::Ambiguous {
                reference,
                candidates,
            } => diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::AmbiguousTable,
                format!(
                    "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    reference,
                    format_table_candidates(&candidates)
                ),
            )),
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    PlannedFile {
        queries,
        diagnostics,
    }
}

fn plan_selection_set(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    root_table: TableId,
    table: TableId,
    clauses: &SelectionClauses,
    selections: &[Selection],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SelectionPlan> {
    let mut items = Vec::new();
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            if let Some(fragment) = resolver.fragment(&selection.name.text)
                && let Some(fragment_plan) = plan_selection_set(
                    catalog,
                    resolver,
                    root_table,
                    table,
                    &SelectionClauses::default(),
                    &fragment.selections,
                    diagnostics,
                )
            {
                items.extend(fragment_plan.items);
            }
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) => {
                if selection.selections.is_empty() {
                    items.push(SelectionPlanItem::Projection(Projection {
                        column: column.id,
                        output_name: selection
                            .alias
                            .as_ref()
                            .map_or_else(|| column.name.clone(), |alias| alias.text.clone()),
                    }));
                }
            }
            FieldCheckResult::Relation(relation) => {
                if let Some(nested) = plan_selection_set(
                    catalog,
                    resolver,
                    root_table,
                    relation.table.id,
                    &plan_clauses(catalog, root_table, relation.table.id, selection),
                    &selection.selections,
                    diagnostics,
                ) {
                    items.push(SelectionPlanItem::Relation(NestedRelation {
                        relation_name: selection.name.text.clone(),
                        output_name: selection
                            .alias
                            .as_ref()
                            .map_or_else(|| relation.name.to_string(), |alias| alias.text.clone()),
                        table: relation.table.id,
                        foreign_key: relation.foreign_key.id,
                        selections: Box::new(nested),
                    }));
                }
            }
            FieldCheckResult::NotFound => {}
            FieldCheckResult::AmbiguousRelation {
                reference,
                candidates,
            } => diagnostics.push(planner_diagnostic(
                selection.name.range,
                DiagnosticCode::AmbiguousRelation,
                format!(
                    "relation `{}` has multiple foreign-key paths; use one of: {}",
                    reference,
                    candidates.join(", ")
                ),
            )),
        }
    }
    Some(SelectionPlan {
        table,
        clauses: clauses.clone(),
        items,
    })
}

fn plan_clauses(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    selection: &Selection,
) -> SelectionClauses {
    let mut clauses = SelectionClauses::default();
    for clause in &selection.clauses {
        match clause {
            crate::Clause::Where(where_clause) => {
                clauses.filter =
                    plan_filter_expr(catalog, root_table, table, &where_clause.predicate);
            }
            crate::Clause::OrderBy(order_by) => {
                clauses
                    .order_by
                    .extend(order_by.items.iter().filter_map(|item| {
                        let crate::FieldCheckResult::Column(column) =
                            catalog.check_field(table, &item.field.text)
                        else {
                            return None;
                        };
                        Some(OrderByPlan {
                            column: column.id,
                            direction: match item.direction {
                                crate::SortDirection::Asc => SortDirectionPlan::Asc,
                                crate::SortDirection::Desc => SortDirectionPlan::Desc,
                            },
                        })
                    }));
            }
            crate::Clause::Limit(limit) => {
                clauses.limit = literal_u64(&limit.value);
            }
            crate::Clause::Offset(offset) => {
                clauses.offset = literal_u64(&offset.value);
            }
        }
    }
    clauses
}

fn plan_filter_expr(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    expr: &crate::Expr,
) -> Option<FilterExpr> {
    match expr {
        crate::Expr::Name(_) => None,
        crate::Expr::Path(path) => plan_filter_path(catalog, root_table, table, path),
        crate::Expr::Literal(literal) => Some(FilterExpr::Literal(match literal {
            crate::Literal::String { value, .. } => FilterLiteral::String(value.clone()),
            crate::Literal::Number { value, .. } => FilterLiteral::Number(value.clone()),
            crate::Literal::Bool { value, .. } => FilterLiteral::Bool(*value),
            crate::Literal::Null { .. } => FilterLiteral::Null,
        })),
        crate::Expr::Binary {
            left, op, right, ..
        } => {
            if let crate::Expr::Path(path) = left.as_ref()
                && let Some(RelationPredicateColumn {
                    foreign_key,
                    table: relation_table,
                    column,
                }) = relation_predicate_column(catalog, table, path)
            {
                return Some(FilterExpr::Exists {
                    foreign_key,
                    table: relation_table,
                    filter: Box::new(FilterExpr::Binary {
                        left: Box::new(FilterExpr::Column {
                            scope: FilterColumnScope::Current,
                            column,
                        }),
                        op: *op,
                        right: Box::new(plan_filter_expr(
                            catalog,
                            root_table,
                            relation_table,
                            right,
                        )?),
                    }),
                });
            }
            Some(FilterExpr::Binary {
                left: Box::new(plan_filter_expr(catalog, root_table, table, left)?),
                op: *op,
                right: Box::new(plan_filter_expr(catalog, root_table, table, right)?),
            })
        }
    }
}

fn plan_filter_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    path: &crate::ScopedPath,
) -> Option<FilterExpr> {
    if path.segments.len() != 1 {
        return None;
    }
    let scope = match path.scope {
        crate::PathScope::Current => FilterColumnScope::Current,
        crate::PathScope::Root => FilterColumnScope::Root,
        crate::PathScope::Parent => return None,
    };
    let source_table = match path.scope {
        crate::PathScope::Current => table,
        crate::PathScope::Root => root_table,
        crate::PathScope::Parent => return None,
    };
    let crate::FieldCheckResult::Column(column) =
        catalog.check_field(source_table, &path.segments[0].field_ref())
    else {
        return None;
    };
    Some(FilterExpr::Column {
        scope,
        column: column.id,
    })
}

struct RelationPredicateColumn {
    foreign_key: ForeignKeyId,
    table: TableId,
    column: crate::ColumnId,
}

fn relation_predicate_column(
    catalog: &Catalog,
    table: TableId,
    path: &crate::ScopedPath,
) -> Option<RelationPredicateColumn> {
    if path.scope != crate::PathScope::Current || path.segments.len() != 2 {
        return None;
    }
    let crate::FieldCheckResult::Relation(relation) =
        catalog.check_field(table, &path.segments[0].field_ref())
    else {
        return None;
    };
    let crate::FieldCheckResult::Column(column) =
        catalog.check_field(relation.table.id, &path.segments[1].field_ref())
    else {
        return None;
    };
    Some(RelationPredicateColumn {
        foreign_key: relation.foreign_key.id,
        table: relation.table.id,
        column: column.id,
    })
}

fn literal_u64(expr: &crate::Expr) -> Option<u64> {
    let crate::Expr::Literal(crate::Literal::Number { value, .. }) = expr else {
        return None;
    };
    value.parse().ok()
}

fn planner_diagnostic(
    range: crate::TextRange,
    code: DiagnosticCode,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        range,
        severity: Severity::Error,
        code,
        message: message.into(),
        source: DiagnosticSource::Check,
    }
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_source;

    fn plan(source: &str) -> PlannedFile {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        plan_file(&parsed.source_file)
    }

    #[test]
    fn plans_scalar_projections_and_nested_relations() {
        let planned = plan("query Q { public.users { id name posts { title } } }");

        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);
        assert_eq!(planned.queries.len(), 1);
        assert_eq!(planned.queries[0].selections.items.len(), 3);
        assert!(matches!(
            &planned.queries[0].selections.items[0],
            SelectionPlanItem::Projection(Projection { output_name, .. }) if output_name == "id"
        ));
        assert!(matches!(
            &planned.queries[0].selections.items[2],
            SelectionPlanItem::Relation(NestedRelation { output_name, .. }) if output_name == "posts"
        ));
    }
}
