use crate::{
    catalog::{Catalog, FieldCheckResult, TableId, TableKey, TableResolution},
    syntax::{
        Argument, Definition, Diagnostic, DiagnosticCode, DiagnosticSource, Document, Expr,
        FragmentDef, Literal, QueryDef, Selection, Severity, SourceFile, TextRange,
    },
};
use facet::Facet;
use indexmap::IndexMap;
use lasso::Rodeo;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Facet)]
#[repr(transparent)]
pub struct NameId(u32);

#[derive(Debug, Default)]
pub struct Interner {
    strings: Rodeo,
    ids: HashMap<String, NameId>,
    values: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, value: &str) -> NameId {
        self.strings.get_or_intern(value);
        if let Some(id) = self.ids.get(value).copied() {
            return id;
        }
        let id = NameId(self.values.len() as u32);
        self.values.push(value.to_string());
        self.ids.insert(value.to_string(), id);
        id
    }

    pub fn resolve(&self, id: NameId) -> Option<&str> {
        self.values.get(id.0 as usize).map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, Facet)]
pub struct NameIndex {
    pub queries: Vec<(String, NameId)>,
    pub fragments: Vec<(String, NameId)>,
    pub fields: Vec<(NameId, TextRange)>,
    pub arguments: Vec<(NameId, TextRange)>,
    pub directives: Vec<(NameId, TextRange)>,
}

#[derive(Clone, Debug, Facet)]
pub struct LoweredFile {
    pub names: NameIndex,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Facet)]
pub struct CheckedFile {
    pub errors: Vec<CheckError>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct CheckError {
    pub range: TextRange,
    pub kind: CheckErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum CheckErrorKind {
    DuplicateFragment {
        name: String,
    },
    TableNotFound {
        table: String,
    },
    AmbiguousTable {
        table: String,
        candidates: Vec<TableKey>,
    },
    FieldNotFound {
        field: String,
        table: String,
    },
    AmbiguousRelation {
        relation: String,
        candidates: Vec<TableKey>,
    },
    DuplicateOutputKey {
        key: String,
    },
    ScalarSelectionSet {
        field: String,
        data_type: String,
    },
    RelationSelectionSet {
        field: String,
    },
}

pub fn lower_file(source_file: &SourceFile, interner: &mut Interner) -> LoweredFile {
    let mut names = NameIndex::default();
    let mut diagnostics = Vec::new();
    lower_document(
        source_file.document(),
        interner,
        &mut names,
        &mut diagnostics,
    );
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    LoweredFile { names, diagnostics }
}

pub fn check_file(source_file: &SourceFile) -> CheckedFile {
    check_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn check_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> CheckedFile {
    let document = source_file.document();
    let mut errors = Vec::new();
    check_queries(document, catalog, &mut errors);
    check_fragments(document, catalog, &mut errors);
    errors.sort_by_key(|error| (error.range.start, error.range.end));
    let mut diagnostics = errors
        .iter()
        .map(|error| error.to_diagnostic())
        .collect::<Vec<_>>();
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    CheckedFile {
        errors,
        diagnostics,
    }
}

impl CheckError {
    pub fn to_diagnostic(&self) -> Diagnostic {
        let (code, message) = match &self.kind {
            CheckErrorKind::DuplicateFragment { name } => (
                DiagnosticCode::DuplicateDefinition,
                format!("duplicate fragment `{name}`"),
            ),
            CheckErrorKind::TableNotFound { table } => (
                DiagnosticCode::TableNotFound,
                format!("table `{table}` not found"),
            ),
            CheckErrorKind::AmbiguousTable { table, candidates } => (
                DiagnosticCode::AmbiguousTable,
                format!(
                    "table `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    table,
                    format_table_candidates(candidates)
                ),
            ),
            CheckErrorKind::FieldNotFound { field, table } => (
                DiagnosticCode::FieldNotFound,
                format!("field `{field}` not found on table `{table}`"),
            ),
            CheckErrorKind::AmbiguousRelation {
                relation,
                candidates,
            } => (
                DiagnosticCode::AmbiguousRelation,
                format!(
                    "relation `{}` is ambiguous; use an alias with a schema-qualified name ({})",
                    relation,
                    format_table_candidates(candidates)
                ),
            ),
            CheckErrorKind::DuplicateOutputKey { key } => (
                DiagnosticCode::DuplicateOutputKey,
                format!("selection output key `{key}` is ambiguous; use an alias"),
            ),
            CheckErrorKind::ScalarSelectionSet { field, data_type } => (
                DiagnosticCode::ScalarSelectionSet,
                format!(
                    "field `{field}` is a scalar ({data_type}) and cannot have a selection set"
                ),
            ),
            CheckErrorKind::RelationSelectionSet { field } => (
                DiagnosticCode::RelationSelectionSet,
                format!("relation field `{field}` must have a selection set"),
            ),
        };
        Diagnostic {
            range: self.range,
            severity: Severity::Error,
            code,
            message,
            source: DiagnosticSource::Check,
        }
    }
}

fn format_table_candidates(candidates: &[TableKey]) -> String {
    candidates
        .iter()
        .map(|candidate| format!("{}.{}", candidate.schema, candidate.table))
        .collect::<Vec<_>>()
        .join(", ")
}

fn check_fragments(document: &Document, catalog: &Catalog, errors: &mut Vec<CheckError>) {
    let mut fragments = IndexMap::<String, TextRange>::new();
    for definition in &document.definitions {
        let Definition::Fragment(fragment) = definition else {
            continue;
        };
        if let Some(name) = &fragment.name
            && fragments.insert(name.text.clone(), name.range).is_some()
        {
            errors.push(CheckError {
                range: name.range,
                kind: CheckErrorKind::DuplicateFragment {
                    name: name.text.clone(),
                },
            });
        }
        check_fragment_selection_set(fragment, catalog, errors);
    }
}

fn check_fragment_selection_set(
    fragment: &FragmentDef,
    catalog: &Catalog,
    errors: &mut Vec<CheckError>,
) {
    let Some(on) = &fragment.on else {
        return;
    };
    let table = match catalog.resolve_table_ref(&on.text) {
        TableResolution::Found(table) => table,
        TableResolution::NotFound { reference } => {
            errors.push(CheckError {
                range: on.range,
                kind: CheckErrorKind::TableNotFound { table: reference },
            });
            return;
        }
        TableResolution::Ambiguous {
            reference,
            candidates,
        } => {
            errors.push(CheckError {
                range: on.range,
                kind: CheckErrorKind::AmbiguousTable {
                    table: reference,
                    candidates,
                },
            });
            return;
        }
    };
    check_selection_set(catalog, table.id, &fragment.selections, errors);
}

fn check_queries(document: &Document, catalog: &Catalog, errors: &mut Vec<CheckError>) {
    for definition in &document.definitions {
        let Definition::Query(query) = definition else {
            continue;
        };
        check_duplicate_output_keys(&query.selections, errors);
        for selection in &query.selections {
            let table = match catalog.resolve_table_ref(&selection.name.text) {
                TableResolution::Found(table) => table,
                TableResolution::NotFound { reference } => {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::TableNotFound { table: reference },
                    });
                    continue;
                }
                TableResolution::Ambiguous {
                    reference,
                    candidates,
                } => {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::AmbiguousTable {
                            table: reference,
                            candidates,
                        },
                    });
                    continue;
                }
            };
            check_selection_set(catalog, table.id, &selection.selections, errors);
        }
    }
}

fn check_selection_set(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    errors: &mut Vec<CheckError>,
) {
    check_duplicate_output_keys(selections, errors);
    for selection in selections {
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) => {
                if !selection.selections.is_empty() {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::ScalarSelectionSet {
                            field: selection.name.text.clone(),
                            data_type: column.data_type.as_str().to_string(),
                        },
                    });
                }
            }
            FieldCheckResult::Relation(relation) => {
                if selection.selections.is_empty() {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::RelationSelectionSet {
                            field: selection.name.text.clone(),
                        },
                    });
                } else {
                    check_selection_set(catalog, relation.table.id, &selection.selections, errors);
                }
            }
            FieldCheckResult::NotFound => {
                let table_name = catalog
                    .tables
                    .get(table.0)
                    .map_or("<unknown>", |table| table.name.as_str());
                errors.push(CheckError {
                    range: selection.name.range,
                    kind: CheckErrorKind::FieldNotFound {
                        field: selection.name.text.clone(),
                        table: table_name.to_string(),
                    },
                });
            }
            FieldCheckResult::AmbiguousRelation {
                reference,
                candidates,
            } => {
                errors.push(CheckError {
                    range: selection.name.range,
                    kind: CheckErrorKind::AmbiguousRelation {
                        relation: reference,
                        candidates,
                    },
                });
            }
        }
    }
}

fn check_duplicate_output_keys(selections: &[Selection], errors: &mut Vec<CheckError>) {
    let mut keys = IndexMap::<String, TextRange>::new();
    for selection in selections {
        let key = response_key(selection);
        if keys.insert(key.clone(), selection.name.range).is_some() {
            errors.push(CheckError {
                range: selection.name.range,
                kind: CheckErrorKind::DuplicateOutputKey { key },
            });
        }
    }
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

fn lower_document(
    document: &Document,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for definition in &document.definitions {
        match definition {
            Definition::Query(query) => lower_query(query, interner, names, diagnostics),
            Definition::Fragment(fragment) => {
                lower_fragment(fragment, interner, names, diagnostics)
            }
        }
    }
}

fn lower_query(
    query: &QueryDef,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(name) = &query.name {
        let id = interner.intern(&name.text);
        if insert_name(&mut names.queries, &name.text, id) {
            diagnostics.push(Diagnostic {
                range: name.range,
                severity: Severity::Error,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!("duplicate query `{}`", name.text),
                source: DiagnosticSource::Lower,
            });
        }
    }
    lower_selections(&query.selections, interner, names);
}

fn lower_fragment(
    fragment: &FragmentDef,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(name) = &fragment.name {
        let id = interner.intern(&name.text);
        if insert_name(&mut names.fragments, &name.text, id) {
            diagnostics.push(Diagnostic {
                range: name.range,
                severity: Severity::Error,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!("duplicate fragment `{}`", name.text),
                source: DiagnosticSource::Lower,
            });
        }
    }
    if let Some(on) = &fragment.on {
        interner.intern(&on.text);
    }
    lower_selections(&fragment.selections, interner, names);
}

fn insert_name(names: &mut Vec<(String, NameId)>, text: &str, id: NameId) -> bool {
    if names.iter().any(|(name, _)| name == text) {
        true
    } else {
        names.push((text.to_string(), id));
        false
    }
}

fn lower_selections(selections: &[Selection], interner: &mut Interner, names: &mut NameIndex) {
    for selection in selections {
        if let Some(alias) = &selection.alias {
            interner.intern(&alias.text);
        }
        names
            .fields
            .push((interner.intern(&selection.name.text), selection.name.range));
        for argument in &selection.arguments {
            lower_argument(argument, interner, names);
        }
        for directive in &selection.directives {
            names
                .directives
                .push((interner.intern(&directive.text), directive.range));
        }
        lower_selections(&selection.selections, interner, names);
    }
}

fn lower_argument(argument: &Argument, interner: &mut Interner, names: &mut NameIndex) {
    names
        .arguments
        .push((interner.intern(&argument.name.text), argument.name.range));
    lower_expr(&argument.value, interner);
}

fn lower_expr(expr: &Expr, interner: &mut Interner) {
    match expr {
        Expr::Name(name) => {
            interner.intern(&name.text);
        }
        Expr::Binary { left, right, .. } => {
            lower_expr(left, interner);
            lower_expr(right, interner);
        }
        Expr::Literal(
            Literal::String { .. }
            | Literal::Number { .. }
            | Literal::Bool { .. }
            | Literal::Null { .. },
        ) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_source;

    fn diagnostics(source: &str) -> Vec<Diagnostic> {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        check_file(&parsed.source_file).diagnostics
    }

    #[test]
    fn hardcoded_catalog_accepts_columns_and_relations() {
        let diagnostics = diagnostics(
            "query Q { public.users { id name posts { title users { name } } } posts { users { email } } }",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn hardcoded_catalog_reports_unknown_table_and_field() {
        let diagnostics = diagnostics("query Q { comments { id } public.users { missing } }");
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::TableNotFound
                && diagnostic.message == "table `comments` not found"
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::FieldNotFound
                && diagnostic.message == "field `missing` not found on table `users`"
        }));
    }

    #[test]
    fn hardcoded_catalog_reports_selection_set_shape_errors() {
        let diagnostics = diagnostics("query Q { public.users { id { name } posts } }");
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::ScalarSelectionSet
                && diagnostic
                    .message
                    .starts_with("field `id` is a scalar (uuid)")
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::RelationSelectionSet
                && diagnostic.message == "relation field `posts` must have a selection set"
        }));
    }

    #[test]
    fn hardcoded_catalog_checks_fragment_fields() {
        let diagnostics = diagnostics("fragment UserFields on public.users { id posts { title } }");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn hardcoded_catalog_accepts_qualified_table_and_relation_names() {
        let diagnostics = diagnostics(
            "query Q { public.users { id public.posts { title } } public.posts { public.users { email } } }",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn hardcoded_catalog_accepts_qualified_fragment_type() {
        let diagnostics =
            diagnostics("fragment UserFields on public.users { id public.posts { title } }");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn unqualified_table_names_default_to_public() {
        let diagnostics = diagnostics("query Q { users { id } }");

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn duplicate_output_keys_require_aliases() {
        let diagnostics = diagnostics("query Q { public.users { id } other_schema.users { id } }");

        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::DuplicateOutputKey);
        assert_eq!(
            diagnostics[0].message,
            "selection output key `users` is ambiguous; use an alias"
        );
    }

    #[test]
    fn aliases_disambiguate_duplicate_output_keys() {
        let diagnostics = diagnostics(
            "query Q { public_users: public.users { id } other_users: other_schema.users { id } }",
        );

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn hardcoded_catalog_reports_fragment_field_errors() {
        let diagnostics = diagnostics("fragment UserFields on public.users { missing }");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::FieldNotFound);
        assert_eq!(
            diagnostics[0].message,
            "field `missing` not found on table `users`"
        );
    }

    #[test]
    fn hardcoded_catalog_reports_unknown_fragment_table() {
        let diagnostics = diagnostics("fragment CommentFields on comments { id }");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::TableNotFound);
        assert_eq!(diagnostics[0].message, "table `comments` not found");
    }
}
