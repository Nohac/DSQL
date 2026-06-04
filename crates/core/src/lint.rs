use crate::{
    catalog::{Catalog, FieldCheckResult, ForeignKey, RelationField, TableId, TableResolution},
    definition::{FragmentMap, FragmentRecord, QueryRecord},
    diagnostics::DsqlDiagnostic,
    syntax::{
        Clause, Definition, DiagnosticCode, DiagnosticSource, Expr, ScopedPath, ScopedPathSegment,
        Selection, SelectionKind, Severity, SourceFile, TextRange, source_span,
    },
};
use facet::Facet;
use miette::LabeledSpan;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct LintedFile {
    pub diagnostics: Vec<LintDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet, thiserror::Error)]
#[repr(C)]
pub enum LintDiagnosticKind {
    #[error("relation `{relation}` joins on unindexed column `{column}`; this can be slow")]
    UnindexedJoinColumn { relation: String, column: String },
    #[error(
        "predicate path `{path}` filters on unindexed column `{column}`; nested scans can be slow"
    )]
    UnindexedScanColumn { path: String, column: String },
    #[error(
        "predicate relation `{relation}` joins on unindexed column `{column}`; nested scans can be slow"
    )]
    UnindexedPredicateJoinColumn { relation: String, column: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct LintDiagnostic {
    pub range: TextRange,
    pub severity: Severity,
    pub kind: LintDiagnosticKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
pub struct LintOptions {
    pub unindexed_scan_severity: Option<Severity>,
}

impl Default for LintOptions {
    fn default() -> Self {
        Self {
            unindexed_scan_severity: Some(Severity::Info),
        }
    }
}

pub type LintedDefinition = LintedFile;

pub fn lint_file(source_file: &SourceFile) -> LintedFile {
    lint_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn lint_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> LintedFile {
    lint_file_with_options(source_file, catalog, LintOptions::default())
}

pub fn lint_file_with_options(
    source_file: &SourceFile,
    catalog: &Catalog,
    options: LintOptions,
) -> LintedFile {
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
                            options,
                            &mut diagnostics,
                        );
                        lint_selection_clauses(
                            catalog,
                            table.id,
                            selection,
                            options,
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
                    lint_selection_set(
                        catalog,
                        table.id,
                        &fragment.selections,
                        options,
                        &mut diagnostics,
                    );
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
    lint_query_definition_with_options(query, _resolver, catalog, LintOptions::default())
}

pub fn lint_query_definition_with_options(
    query: &QueryRecord,
    _resolver: &FragmentMap,
    catalog: &Catalog,
    options: LintOptions,
) -> LintedDefinition {
    let mut diagnostics = Vec::new();
    for selection in &query.selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        if let TableResolution::Found(table) = catalog.resolve_table_ref(&selection.name.text) {
            lint_selection_set(
                catalog,
                table.id,
                &selection.selections,
                options,
                &mut diagnostics,
            );
            lint_selection_clauses(catalog, table.id, selection, options, &mut diagnostics);
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
    lint_fragment_definition_with_options(fragment, _resolver, catalog, LintOptions::default())
}

pub fn lint_fragment_definition_with_options(
    fragment: &FragmentRecord,
    _resolver: &FragmentMap,
    catalog: &Catalog,
    options: LintOptions,
) -> LintedDefinition {
    let mut diagnostics = Vec::new();
    if let Some(on) = &fragment.on
        && let TableResolution::Found(table) = catalog.resolve_table_ref(on)
    {
        lint_selection_set(
            catalog,
            table.id,
            &fragment.selections,
            options,
            &mut diagnostics,
        );
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    LintedFile { diagnostics }
}

fn lint_selection_set(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    options: LintOptions,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        if let FieldCheckResult::Relation(relation) =
            catalog.check_field(table, &selection.name.text)
        {
            lint_relation_indexes(catalog, &relation, selection, options, diagnostics);
            lint_selection_clauses(catalog, relation.table.id, selection, options, diagnostics);
            lint_selection_set(
                catalog,
                relation.table.id,
                &selection.selections,
                options,
                diagnostics,
            );
        }
    }
}

fn lint_relation_indexes(
    catalog: &Catalog,
    relation: &RelationField<'_>,
    selection: &Selection,
    options: LintOptions,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let Some(severity) = options.unindexed_scan_severity else {
        return;
    };
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
        diagnostics.push(LintDiagnostic {
            range: selection.name.range,
            severity,
            kind: LintDiagnosticKind::UnindexedJoinColumn {
                relation: selection.name.text.clone(),
                column: catalog.table_by_id(column.table).map_or_else(
                    || column.name.clone(),
                    |table| format!("{}.{}", table.name, column.name),
                ),
            },
        });
    }
}

fn lint_selection_clauses(
    catalog: &Catalog,
    table: TableId,
    selection: &Selection,
    options: LintOptions,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let Some(severity) = options.unindexed_scan_severity else {
        return;
    };
    for clause in &selection.clauses {
        if let Clause::Where(where_clause) = clause {
            lint_expr_predicate_indexes(
                catalog,
                table,
                &where_clause.predicate,
                severity,
                diagnostics,
            );
        }
    }
}

fn lint_expr_predicate_indexes(
    catalog: &Catalog,
    table: TableId,
    expr: &Expr,
    severity: Severity,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    match expr {
        Expr::Path(path) => {
            lint_predicate_path_indexes(catalog, table, path, severity, diagnostics)
        }
        Expr::Binary { left, right, .. } => {
            lint_expr_predicate_indexes(catalog, table, left, severity, diagnostics);
            lint_expr_predicate_indexes(catalog, table, right, severity, diagnostics);
        }
        Expr::Name(_) | Expr::Variable(_) | Expr::Literal(_) => {}
    }
}

fn lint_predicate_path_indexes(
    catalog: &Catalog,
    table: TableId,
    path: &ScopedPath,
    severity: Severity,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if path.scope != crate::PathScope::Current || path.segments.len() < 2 {
        return;
    }

    let mut current_table = table;
    let Some((last, relations)) = path.segments.split_last() else {
        return;
    };
    for relation_segment in relations {
        let FieldCheckResult::Relation(relation) =
            catalog.check_field(current_table, &relation_segment.field_ref())
        else {
            return;
        };
        lint_foreign_key_indexes(
            catalog,
            relation.foreign_key,
            relation_segment,
            severity,
            diagnostics,
        );
        current_table = relation.table.id;
    }

    let FieldCheckResult::Column(column) = catalog.check_field(current_table, &last.field_ref())
    else {
        return;
    };
    if column.is_indexed {
        return;
    }
    diagnostics.push(LintDiagnostic {
        range: last.range,
        severity,
        kind: LintDiagnosticKind::UnindexedScanColumn {
            path: predicate_path_label(path),
            column: catalog.table_by_id(column.table).map_or_else(
                || column.name.clone(),
                |table| format!("{}.{}", table.name, column.name),
            ),
        },
    });
}

fn lint_foreign_key_indexes(
    catalog: &Catalog,
    foreign_key: &ForeignKey,
    segment: &ScopedPathSegment,
    severity: Severity,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    for column_id in foreign_key
        .from_columns
        .iter()
        .chain(foreign_key.to_columns.iter())
    {
        let Some(column) = catalog.column_by_id(*column_id) else {
            continue;
        };
        if column.is_indexed {
            continue;
        }
        diagnostics.push(LintDiagnostic {
            range: segment.range,
            severity,
            kind: LintDiagnosticKind::UnindexedPredicateJoinColumn {
                relation: segment.field_ref(),
                column: catalog.table_by_id(column.table).map_or_else(
                    || column.name.clone(),
                    |table| format!("{}.{}", table.name, column.name),
                ),
            },
        });
    }
}

fn predicate_path_label(path: &ScopedPath) -> String {
    let prefix = match path.scope {
        crate::PathScope::Current => ".",
        crate::PathScope::Parent => "..",
        crate::PathScope::Root => "~",
    };
    format!(
        "{}{}",
        prefix,
        path.segments
            .iter()
            .map(|segment| segment.field_ref())
            .collect::<Vec<_>>()
            .join(".")
    )
}

impl fmt::Display for LintDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for LintDiagnostic {}

impl miette::Diagnostic for LintDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("{:?}", DsqlDiagnostic::code(self))))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
            Severity::Info => miette::Severity::Advice,
        })
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::underline(
            source_span(self.range),
        ))))
    }
}

impl DsqlDiagnostic for LintDiagnostic {
    fn range(&self) -> TextRange {
        self.range
    }

    fn severity(&self) -> Severity {
        self.severity
    }

    fn code(&self) -> DiagnosticCode {
        match self.kind {
            LintDiagnosticKind::UnindexedJoinColumn { .. }
            | LintDiagnosticKind::UnindexedPredicateJoinColumn { .. } => {
                DiagnosticCode::UnindexedJoinColumn
            }
            LintDiagnosticKind::UnindexedScanColumn { .. } => DiagnosticCode::UnindexedScanColumn,
        }
    }

    fn source(&self) -> DiagnosticSource {
        DiagnosticSource::Lint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DsqlDiagnostic;
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

    fn lint_with_options(source: &str, options: LintOptions) -> LintedFile {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        lint_file_with_options(&parsed.source_file, &Catalog::hardcoded(), options)
    }

    #[test]
    fn reports_unindexed_join_columns_for_relation_selections() {
        let lint = lint("query Q { public.users { posts { title } } }");

        assert_eq!(lint.diagnostics.len(), 1, "{:?}", lint.diagnostics);
        let diagnostic = lint.diagnostics[0].to_transport();
        assert_eq!(diagnostic.code, DiagnosticCode::UnindexedJoinColumn);
        assert_eq!(diagnostic.severity, Severity::Info);
        assert!(diagnostic.message.contains("posts.user_id"));
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
                .map(DsqlDiagnostic::to_transport)
                .all(|diagnostic| diagnostic.code == DiagnosticCode::UnindexedJoinColumn)
        );
    }

    #[test]
    fn reports_unindexed_predicate_relationship_scans() {
        let lint = lint("query Q { public.users(where .posts.title == \"foo\") { id } }");

        assert!(
            lint.diagnostics
                .iter()
                .map(DsqlDiagnostic::to_transport)
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UnindexedScanColumn),
            "{:?}",
            lint.diagnostics
        );
        assert!(
            lint.diagnostics
                .iter()
                .map(DsqlDiagnostic::to_transport)
                .any(|diagnostic| diagnostic.message.contains("posts.title")),
            "{:?}",
            lint.diagnostics
        );
    }

    #[test]
    fn unindexed_scan_lint_severity_is_configurable() {
        let warning = lint_with_options(
            "query Q { public.users(where .posts.title == \"foo\") { id } }",
            LintOptions {
                unindexed_scan_severity: Some(Severity::Warning),
            },
        );
        assert!(
            warning
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity == Severity::Warning),
            "{:?}",
            warning.diagnostics
        );

        let off = lint_with_options(
            "query Q { public.users(where .posts.title == \"foo\") { id } }",
            LintOptions {
                unindexed_scan_severity: None,
            },
        );
        assert!(off.diagnostics.is_empty(), "{:?}", off.diagnostics);
    }
}
