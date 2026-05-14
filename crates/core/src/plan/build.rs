use super::{NestedRelation, PlannedFile, Projection, QueryPlan, SelectionPlan};
use crate::{
    catalog::{Catalog, FieldCheckResult, TableId, TableKey, TableResolution},
    definition::{DefinitionResolver, FragmentMap, extract_definitions},
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
                    if let Some(selections) = plan_selection_set(
                        catalog,
                        &resolver,
                        table.id,
                        &selection.selections,
                        &mut diagnostics,
                    ) {
                        queries.push(QueryPlan {
                            root: table.id,
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

fn plan_selection_set(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    table: TableId,
    selections: &[Selection],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SelectionPlan> {
    let mut projections = Vec::new();
    let mut relations = Vec::new();
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            if let Some(fragment) = resolver.fragment(&selection.name.text)
                && let Some(fragment_plan) =
                    plan_selection_set(catalog, resolver, table, &fragment.selections, diagnostics)
            {
                projections.extend(fragment_plan.projections);
                relations.extend(fragment_plan.relations);
            }
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) => {
                if selection.selections.is_empty() {
                    projections.push(Projection {
                        column: column.id,
                        alias: selection.alias.as_ref().map(|alias| alias.text.clone()),
                    });
                }
            }
            FieldCheckResult::Relation(relation) => {
                if let Some(nested) = plan_selection_set(
                    catalog,
                    resolver,
                    relation.table.id,
                    &selection.selections,
                    diagnostics,
                ) {
                    relations.push(NestedRelation {
                        field_name: selection.name.text.clone(),
                        table: relation.table.id,
                        foreign_key: relation.foreign_key.id,
                        selections: Box::new(nested),
                    });
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
                    "relation `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    reference,
                    format_table_candidates(&candidates)
                ),
            )),
        }
    }
    Some(SelectionPlan {
        table,
        projections,
        relations,
    })
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
        assert_eq!(planned.queries[0].selections.projections.len(), 2);
        assert_eq!(planned.queries[0].selections.relations.len(), 1);
        assert_eq!(
            planned.queries[0].selections.relations[0].field_name,
            "posts"
        );
    }
}
