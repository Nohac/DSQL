use super::{CheckError, CheckErrorKind, CheckedFile};
use crate::{
    catalog::{Catalog, FieldCheckResult, LiteralKind, TableId, TableResolution},
    definition::{
        DefinitionResolver, FragmentMap, FragmentRecord, QueryRecord, extract_definitions,
    },
    syntax::{
        Clause, Definition, Document, Expr, Literal, Selection, SelectionKind, SourceFile,
        TextRange,
    },
};
use indexmap::IndexMap;
use std::collections::HashSet;

pub fn check_file(source_file: &SourceFile) -> CheckedFile {
    check_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn check_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> CheckedFile {
    let extracted = extract_definitions(source_file);
    let resolver = FragmentMap::from_file(&extracted);
    let mut errors = Vec::new();
    check_duplicate_fragments(source_file.document(), &mut errors);
    for definition in &extracted.definitions {
        match definition {
            crate::DefinitionRecord::Query(query) => {
                errors.extend(check_query_definition(query, &resolver, catalog).errors);
            }
            crate::DefinitionRecord::Fragment(fragment) => {
                errors.extend(check_fragment_definition(fragment, &resolver, catalog).errors);
            }
        }
    }
    checked(errors)
}

pub fn check_query_definition(
    query: &QueryRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> CheckedFile {
    let mut errors = Vec::new();
    check_root_selections(catalog, resolver, &query.selections, &mut errors);
    checked(errors)
}

pub fn check_fragment_definition(
    fragment: &FragmentRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
) -> CheckedFile {
    let mut errors = Vec::new();
    check_fragment_record(
        fragment,
        resolver,
        catalog,
        &mut errors,
        &mut HashSet::new(),
    );
    checked(errors)
}

fn checked(mut errors: Vec<CheckError>) -> CheckedFile {
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

fn check_duplicate_fragments(document: &Document, errors: &mut Vec<CheckError>) {
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
    }
}

fn check_fragment_record(
    fragment: &FragmentRecord,
    resolver: &impl DefinitionResolver,
    catalog: &Catalog,
    errors: &mut Vec<CheckError>,
    visiting: &mut HashSet<String>,
) {
    let Some(on) = &fragment.on else {
        return;
    };
    let range = fragment.on_range.unwrap_or(fragment.range);
    let table = match catalog.resolve_table_ref(on) {
        TableResolution::Found(table) => table,
        TableResolution::NotFound { reference } => {
            errors.push(CheckError {
                range,
                kind: CheckErrorKind::TableNotFound { table: reference },
            });
            return;
        }
        TableResolution::Ambiguous {
            reference,
            candidates,
        } => {
            errors.push(CheckError {
                range,
                kind: CheckErrorKind::AmbiguousTable {
                    table: reference,
                    candidates,
                },
            });
            return;
        }
    };
    check_selection_set(
        catalog,
        resolver,
        table.id,
        &fragment.selections,
        errors,
        visiting,
    );
}

fn check_root_selections(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    selections: &[Selection],
    errors: &mut Vec<CheckError>,
) {
    check_duplicate_output_keys(selections, errors);
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            errors.push(CheckError {
                range: selection.name.range,
                kind: CheckErrorKind::UnknownFragment {
                    fragment: selection.name.text.clone(),
                },
            });
            continue;
        }
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
        check_clauses(catalog, table.id, selection, errors);
        check_selection_set(
            catalog,
            resolver,
            table.id,
            &selection.selections,
            errors,
            &mut HashSet::new(),
        );
    }
}

fn check_selection_set(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    table: TableId,
    selections: &[Selection],
    errors: &mut Vec<CheckError>,
    visiting: &mut HashSet<String>,
) {
    check_duplicate_output_keys(selections, errors);
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            check_fragment_spread(catalog, resolver, table, selection, errors, visiting);
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) => {
                if selection.has_clause_list {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::ScalarClauses {
                            field: selection.name.text.clone(),
                            data_type: column.data_type.as_str().to_string(),
                        },
                    });
                }
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
                check_clauses(catalog, relation.table.id, selection, errors);
                if selection.selections.is_empty() {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::RelationSelectionSet {
                            field: selection.name.text.clone(),
                        },
                    });
                } else {
                    check_selection_set(
                        catalog,
                        resolver,
                        relation.table.id,
                        &selection.selections,
                        errors,
                        visiting,
                    );
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

fn check_clauses(
    catalog: &Catalog,
    table: TableId,
    selection: &Selection,
    errors: &mut Vec<CheckError>,
) {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                check_predicate_expr(catalog, table, &where_clause.predicate, errors)
            }
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    if !matches!(
                        catalog.check_field(table, &item.field.text),
                        FieldCheckResult::Column(_)
                    ) {
                        let table_name = catalog
                            .tables
                            .get(table.0)
                            .map_or("<unknown>", |table| table.name.as_str());
                        errors.push(CheckError {
                            range: item.field.range,
                            kind: CheckErrorKind::FieldNotFound {
                                field: item.field.text.clone(),
                                table: table_name.to_string(),
                            },
                        });
                    }
                }
            }
            Clause::Limit(limit) => {
                check_non_negative_integer("limit", &limit.value, limit.range, errors)
            }
            Clause::Offset(offset) => {
                check_non_negative_integer("offset", &offset.value, offset.range, errors);
            }
        }
    }
}

fn check_predicate_expr(
    catalog: &Catalog,
    table: TableId,
    expr: &Expr,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::Name(name) => {
            if !matches!(
                catalog.check_field(table, &name.text),
                FieldCheckResult::Column(_)
            ) {
                let table_name = catalog
                    .tables
                    .get(table.0)
                    .map_or("<unknown>", |table| table.name.as_str());
                errors.push(CheckError {
                    range: name.range,
                    kind: CheckErrorKind::FieldNotFound {
                        field: name.text.clone(),
                        table: table_name.to_string(),
                    },
                });
            }
        }
        Expr::Binary { left, right, .. } => {
            check_binary_predicate_types(catalog, table, left, right, errors);
            check_predicate_expr(catalog, table, left, errors);
            check_predicate_expr(catalog, table, right, errors);
        }
        Expr::Literal(_) => {}
    }
}

fn check_binary_predicate_types(
    catalog: &Catalog,
    table: TableId,
    left: &Expr,
    right: &Expr,
    errors: &mut Vec<CheckError>,
) {
    let (name, literal) = match (left, right) {
        (Expr::Name(name), Expr::Literal(literal)) => (name, literal),
        (Expr::Literal(literal), Expr::Name(name)) => (name, literal),
        _ => return,
    };
    let FieldCheckResult::Column(column) = catalog.check_field(table, &name.text) else {
        return;
    };
    let actual = literal_kind(literal);
    if actual == LiteralKind::Null {
        return;
    }
    if !column
        .data_type
        .accepts_literal_value(actual, literal_value(literal))
    {
        errors.push(CheckError {
            range: literal_range(literal),
            kind: CheckErrorKind::PredicateTypeMismatch {
                field: name.text.clone(),
                expected: column.data_type,
                actual,
            },
        });
    }
}

fn literal_kind(literal: &Literal) -> LiteralKind {
    match literal {
        Literal::String { .. } => LiteralKind::String,
        Literal::Number { .. } => LiteralKind::Number,
        Literal::Bool { .. } => LiteralKind::Boolean,
        Literal::Null { .. } => LiteralKind::Null,
    }
}

fn literal_value(literal: &Literal) -> &str {
    match literal {
        Literal::String { value, .. } | Literal::Number { value, .. } => value,
        Literal::Bool { value, .. } => {
            if *value {
                "true"
            } else {
                "false"
            }
        }
        Literal::Null { .. } => "null",
    }
}

fn literal_range(literal: &Literal) -> TextRange {
    match literal {
        Literal::String { range, .. }
        | Literal::Number { range, .. }
        | Literal::Bool { range, .. }
        | Literal::Null { range } => *range,
    }
}

fn check_non_negative_integer(
    clause: &str,
    expr: &Expr,
    range: TextRange,
    errors: &mut Vec<CheckError>,
) {
    let valid = matches!(
        expr,
        Expr::Literal(Literal::Number { value, .. }) if value.parse::<u64>().is_ok()
    );
    if !valid {
        errors.push(CheckError {
            range,
            kind: CheckErrorKind::ClauseValueTypeMismatch {
                clause: clause.to_string(),
                expected: "a non-negative integer".to_string(),
            },
        });
    }
}

fn check_fragment_spread(
    catalog: &Catalog,
    resolver: &impl DefinitionResolver,
    table: TableId,
    selection: &Selection,
    errors: &mut Vec<CheckError>,
    visiting: &mut HashSet<String>,
) {
    let name = &selection.name.text;
    let Some(fragment) = resolver.fragment(name) else {
        errors.push(CheckError {
            range: selection.name.range,
            kind: CheckErrorKind::UnknownFragment {
                fragment: name.clone(),
            },
        });
        return;
    };
    if !visiting.insert(name.clone()) {
        errors.push(CheckError {
            range: selection.name.range,
            kind: CheckErrorKind::CircularFragmentSpread {
                fragment: name.clone(),
            },
        });
        return;
    }
    let Some(on) = &fragment.on else {
        visiting.remove(name);
        return;
    };
    let fragment_table = match catalog.resolve_table_ref(on) {
        TableResolution::Found(fragment_table) => fragment_table,
        TableResolution::NotFound { reference } => {
            errors.push(CheckError {
                range: fragment.on_range.unwrap_or(fragment.range),
                kind: CheckErrorKind::TableNotFound { table: reference },
            });
            visiting.remove(name);
            return;
        }
        TableResolution::Ambiguous {
            reference,
            candidates,
        } => {
            errors.push(CheckError {
                range: fragment.on_range.unwrap_or(fragment.range),
                kind: CheckErrorKind::AmbiguousTable {
                    table: reference,
                    candidates,
                },
            });
            visiting.remove(name);
            return;
        }
    };
    if fragment_table.id != table {
        let expected = catalog
            .table_by_id(table)
            .map_or_else(|| "<unknown>".to_string(), |table| table.key.table.clone());
        errors.push(CheckError {
            range: selection.name.range,
            kind: CheckErrorKind::FragmentTypeMismatch {
                fragment: name.clone(),
                expected,
                actual: fragment_table.key.table.clone(),
            },
        });
        visiting.remove(name);
        return;
    }
    visiting.remove(name);
}

fn check_duplicate_output_keys(selections: &[Selection], errors: &mut Vec<CheckError>) {
    let mut keys = IndexMap::<String, TextRange>::new();
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
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
    let name = name.split_once("::").map_or(name, |(name, _)| name);
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
        let diagnostics =
            diagnostics("query Q { public.users { id { name } name(where id == 1) posts } }");
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::ScalarSelectionSet
                && diagnostic
                    .message
                    .starts_with("field `id` is a scalar (uuid)")
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::ScalarClauses
                && diagnostic.message
                    == "field `name` is a scalar (text); only relations can have clauses"
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
    fn fragment_spreads_are_checked_in_query_context() {
        let diagnostics = diagnostics(
            "fragment UserFields on public.users { id posts { title } }\nquery Q { users { ...UserFields } }",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn unknown_fragment_spreads_are_reported() {
        let diagnostics = diagnostics("query Q { users { ...MissingFields } }");

        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::UnknownFragment);
        assert_eq!(diagnostics[0].message, "fragment `MissingFields` not found");
    }

    #[test]
    fn fragment_spreads_must_match_current_table() {
        let diagnostics = diagnostics(
            "fragment PostFields on posts { title }\nquery Q { users { ...PostFields } }",
        );

        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::FragmentTypeMismatch);
    }

    #[test]
    fn hardcoded_catalog_accepts_qualified_table_and_relation_names() {
        let diagnostics = diagnostics(
            "query Q { public.users { id public.posts { title } } public.posts { public.users { email } } }",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn relation_clauses_are_checked_against_related_table() {
        let diagnostics = diagnostics(
            "query Q { public.users { posts(where title == \"hello\" order by title) { id } } }",
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
