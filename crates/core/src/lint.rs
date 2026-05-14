use crate::{
    catalog::{Catalog, FieldCheckResult, RelationField, TableId, TableResolution},
    definition::{FragmentMap, FragmentRecord, QueryRecord},
    syntax::{
        Definition, Diagnostic, DiagnosticCode, DiagnosticSource, Selection, SelectionKind,
        Severity, SourceFile,
    },
};
use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct LintedFile {
    pub diagnostics: Vec<Diagnostic>,
}

pub type LintedDefinition = LintedFile;

pub fn lint_file(source_file: &SourceFile) -> LintedFile {
    lint_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn lint_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> LintedFile {
    let mut diagnostics = Vec::new();
    for definition in source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if let TableResolution::Found(table) =
                        catalog.resolve_table_ref(&selection.name.text)
                    {
                        lint_selection_set(
                            catalog,
                            table.id,
                            &selection.selections,
                            &mut diagnostics,
                        );
                    }
                }
            }
            Definition::Fragment(fragment) => {
                let Some(on) = &fragment.on else {
                    continue;
                };
                if let TableResolution::Found(table) = catalog.resolve_table_ref(&on.text) {
                    lint_selection_set(catalog, table.id, &fragment.selections, &mut diagnostics);
                }
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    LintedFile { diagnostics }
}

pub fn lint_query_definition(
    query: &QueryRecord,
    _resolver: &FragmentMap,
    catalog: &Catalog,
) -> LintedDefinition {
    let mut diagnostics = Vec::new();
    for selection in &query.selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        if let TableResolution::Found(table) = catalog.resolve_table_ref(&selection.name.text) {
            lint_selection_set(catalog, table.id, &selection.selections, &mut diagnostics);
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    LintedFile { diagnostics }
}

pub fn lint_fragment_definition(
    fragment: &FragmentRecord,
    _resolver: &FragmentMap,
    catalog: &Catalog,
) -> LintedDefinition {
    let mut diagnostics = Vec::new();
    if let Some(on) = &fragment.on
        && let TableResolution::Found(table) = catalog.resolve_table_ref(on)
    {
        lint_selection_set(catalog, table.id, &fragment.selections, &mut diagnostics);
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    LintedFile { diagnostics }
}

fn lint_selection_set(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        if let FieldCheckResult::Relation(relation) =
            catalog.check_field(table, &selection.name.text)
        {
            lint_relation_indexes(catalog, &relation, selection, diagnostics);
            lint_selection_set(
                catalog,
                relation.table.id,
                &selection.selections,
                diagnostics,
            );
        }
    }
}

fn lint_relation_indexes(
    catalog: &Catalog,
    relation: &RelationField<'_>,
    selection: &Selection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for column_id in relation
        .foreign_key
        .from_columns
        .iter()
        .chain(relation.foreign_key.to_columns.iter())
    {
        let Some(column) = catalog.column_by_id(*column_id) else {
            continue;
        };
        if column.is_indexed {
            continue;
        }
        diagnostics.push(Diagnostic {
            range: selection.name.range,
            severity: Severity::Info,
            code: DiagnosticCode::UnindexedJoinColumn,
            message: format!(
                "relation `{}` joins on unindexed column `{}`; this can be slow",
                selection.name.text,
                catalog.table_by_id(column.table).map_or_else(
                    || column.name.clone(),
                    |table| format!("{}.{}", table.name, column.name)
                )
            ),
            source: DiagnosticSource::Lint,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_source;

    fn lint(source: &str) -> LintedFile {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        lint_file(&parsed.source_file)
    }

    #[test]
    fn reports_unindexed_join_columns_for_relation_selections() {
        let lint = lint("query Q { public.users { posts { title } } }");

        assert_eq!(lint.diagnostics.len(), 1, "{:?}", lint.diagnostics);
        assert_eq!(
            lint.diagnostics[0].code,
            DiagnosticCode::UnindexedJoinColumn
        );
        assert_eq!(lint.diagnostics[0].severity, Severity::Info);
        assert!(lint.diagnostics[0].message.contains("posts.user_id"));
    }

    #[test]
    fn reports_unindexed_join_columns_inside_fragment_spreads() {
        let lint = lint(
            "fragment UserFields on public.users { posts { title } }\nquery Q { public.users { ...UserFields } }",
        );

        assert_eq!(lint.diagnostics.len(), 1, "{:?}", lint.diagnostics);
        assert!(
            lint.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == DiagnosticCode::UnindexedJoinColumn)
        );
    }
}
