use super::{CheckError, CheckErrorKind, CheckedFile};
use crate::{
    catalog::{Catalog, FieldCheckResult, TableId, TableResolution},
    syntax::{Definition, Document, FragmentDef, Selection, SourceFile, TextRange},
};
use indexmap::IndexMap;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{Diagnostic, DiagnosticCode, parse_source};

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
