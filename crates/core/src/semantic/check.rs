use super::{CheckError, CheckErrorKind, CheckedFile};
use crate::{
    catalog::{Catalog, FieldCheckResult, LiteralKind, TableId, TableResolution},
    definition::{
        DefinitionResolver, FragmentMap, FragmentRecord, QueryRecord, extract_definitions,
    },
    syntax::{Clause, Expr, Literal, Selection, SelectionKind, SourceFile, TextRange},
};
use indexmap::IndexMap;
use std::collections::HashSet;

const POSTGRES_RESULT_ALIAS_MAX_BYTES: usize = 63;

pub fn check_file(source_file: &SourceFile) -> CheckedFile {
    check_file_with_catalog(source_file, &Catalog::hardcoded())
}

pub fn check_file_with_catalog(source_file: &SourceFile, catalog: &Catalog) -> CheckedFile {
    let extracted = extract_definitions(source_file);
    let resolver = FragmentMap::from_file(&extracted);
    let mut errors = Vec::new();
    errors.extend(duplicate_fragment_errors(&resolver));
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
    let mut diagnostics = errors.clone();
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    CheckedFile {
        errors,
        diagnostics,
    }
}

pub fn duplicate_fragment_errors(fragments: &FragmentMap) -> Vec<CheckError> {
    fragments
        .duplicate_fragments()
        .into_iter()
        .map(|fragment| CheckError {
            range: fragment.name_range,
            kind: CheckErrorKind::DuplicateFragment {
                name: fragment.key.name.clone(),
            },
        })
        .collect()
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
    let table = match catalog.resolve_table_ref_for(on) {
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
    check_output_key_lengths(selections, errors);
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            errors.push(CheckError {
                range: selection.name.range,
                kind: CheckErrorKind::UnknownFragment {
                    fragment: selection.name.target.name.text.clone(),
                },
            });
            continue;
        }
        let table = match catalog.resolve_table_ref_for(&selection.name.target) {
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
        check_clauses(catalog, table.id, table.id, selection, errors);
        check_selection_set(
            catalog,
            resolver,
            table.id,
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
    root_table: TableId,
    table: TableId,
    selections: &[Selection],
    errors: &mut Vec<CheckError>,
    visiting: &mut HashSet<String>,
) {
    check_duplicate_output_keys(selections, errors);
    check_output_key_lengths(selections, errors);
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            check_fragment_spread(catalog, resolver, table, selection, errors, visiting);
            continue;
        }
        match catalog.check_field_ref(table, &selection.name) {
            FieldCheckResult::Column(column) => {
                if selection.has_clause_list {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::ScalarClauses {
                            field: selection.name.display_text(),
                            data_type: column.data_type.as_str().to_string(),
                        },
                    });
                }
                if !selection.selections.is_empty() {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::ScalarSelectionSet {
                            field: selection.name.display_text(),
                            data_type: column.data_type.as_str().to_string(),
                        },
                    });
                }
            }
            FieldCheckResult::Relation(relation) => {
                check_clauses(catalog, root_table, relation.table.id, selection, errors);
                if selection.selections.is_empty() {
                    errors.push(CheckError {
                        range: selection.name.range,
                        kind: CheckErrorKind::RelationSelectionSet {
                            field: selection.name.display_text(),
                        },
                    });
                } else {
                    check_selection_set(
                        catalog,
                        resolver,
                        root_table,
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
                        field: selection.name.display_text(),
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
    root_table: TableId,
    table: TableId,
    selection: &Selection,
    errors: &mut Vec<CheckError>,
) {
    for clause in &selection.clauses {
        match clause {
            Clause::Where(where_clause) => {
                check_predicate_expr(catalog, root_table, table, &where_clause.predicate, errors)
            }
            Clause::OrderBy(order_by) => {
                for item in &order_by.items {
                    if !matches!(
                        catalog.check_field_ref(
                            table,
                            &crate::RelationRef {
                                range: item.field.range,
                                target: item.field.clone(),
                                selector: None,
                            },
                        ),
                        FieldCheckResult::Column(_)
                    ) {
                        let table_name = catalog
                            .tables
                            .get(table.0)
                            .map_or("<unknown>", |table| table.name.as_str());
                        errors.push(CheckError {
                            range: item.field.range,
                            kind: CheckErrorKind::FieldNotFound {
                                field: item.field.display_text(),
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
    root_table: TableId,
    table: TableId,
    expr: &Expr,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::Name(name) => {
            errors.push(CheckError {
                range: name.range,
                kind: CheckErrorKind::FieldNotFound {
                    field: name.text.clone(),
                    table: table_name(catalog, table).to_string(),
                },
            });
        }
        Expr::Path(path) => {
            if resolve_predicate_path(catalog, root_table, table, path).is_none() {
                errors.push(CheckError {
                    range: path.range,
                    kind: CheckErrorKind::FieldNotFound {
                        field: predicate_path_label(path),
                        table: table_name(catalog, table).to_string(),
                    },
                });
            }
        }
        Expr::Binary {
            left, op, right, ..
        } => {
            match op {
                crate::BinaryOperator::Static(op) if is_comparison_op(*op) => {
                    check_binary_predicate_types(
                        catalog, root_table, table, left, *op, right, errors,
                    );
                }
                crate::BinaryOperator::Variable(operator) => {
                    check_operator_variable(
                        catalog, root_table, table, left, right, operator, errors,
                    );
                }
                crate::BinaryOperator::Static(_) => {}
            }
            check_predicate_expr(catalog, root_table, table, left, errors);
            check_predicate_expr(catalog, root_table, table, right, errors);
        }
        Expr::Variable(_) => {}
        Expr::Literal(_) => {}
    }
}

fn check_operator_variable(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    left: &Expr,
    right: &Expr,
    operator: &crate::OperatorVariable,
    errors: &mut Vec<CheckError>,
) {
    let path = match (left, right) {
        (Expr::Path(path), _) | (_, Expr::Path(path)) => path,
        _ => return,
    };
    let Some(data_type) = resolve_predicate_path(catalog, root_table, table, path) else {
        return;
    };
    for allowed in &operator.allowed {
        if !operator_allowed_for_type(data_type, *allowed) {
            errors.push(CheckError {
                range: operator.range,
                kind: CheckErrorKind::ClauseValueTypeMismatch {
                    clause: "operator".to_string(),
                    expected: format!("an operator valid for {}", data_type.as_str()),
                },
            });
        }
    }
}

fn operator_allowed_for_type(data_type: crate::DataType, op: crate::BinaryOp) -> bool {
    data_type.operator_ops().contains(&op)
}

fn is_comparison_op(op: crate::BinaryOp) -> bool {
    op.is_comparison()
}

fn check_binary_predicate_types(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    left: &Expr,
    op: crate::BinaryOp,
    right: &Expr,
    errors: &mut Vec<CheckError>,
) {
    let (path, literal) = match (left, right) {
        (Expr::Path(path), Expr::Literal(literal)) => (path, literal),
        (Expr::Literal(literal), Expr::Path(path)) => (path, literal),
        _ => return,
    };
    let Some(data_type) = resolve_predicate_path(catalog, root_table, table, path) else {
        return;
    };
    let actual = literal_kind(literal);
    if actual == LiteralKind::Null {
        return;
    }
    if op == crate::BinaryOp::Like && data_type != crate::DataType::Text {
        errors.push(CheckError {
            range: literal_range(literal),
            kind: CheckErrorKind::PredicateTypeMismatch {
                field: predicate_path_label(path),
                expected: crate::DataType::Text,
                actual,
            },
        });
        return;
    }
    if !data_type.accepts_literal_value(actual, literal_value(literal)) {
        errors.push(CheckError {
            range: literal_range(literal),
            kind: CheckErrorKind::PredicateTypeMismatch {
                field: predicate_path_label(path),
                expected: data_type,
                actual,
            },
        });
    }
}

fn resolve_predicate_path(
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    path: &crate::ScopedPath,
) -> Option<crate::DataType> {
    let (mut current_table, segments) = match path.scope {
        crate::PathScope::Current => (table, path.segments.as_slice()),
        crate::PathScope::Root => (root_table, path.segments.as_slice()),
        crate::PathScope::Parent => return None,
    };
    let (last, relations) = segments.split_last()?;
    for relation_ref in relations {
        let FieldCheckResult::Relation(relation) =
            catalog.check_field_ref(current_table, &relation_ref.relation_ref())
        else {
            return None;
        };
        current_table = relation.table.id;
    }
    let FieldCheckResult::Column(column) =
        catalog.check_field_ref(current_table, &last.relation_ref())
    else {
        return None;
    };
    Some(column.data_type)
}

fn predicate_path_label(path: &crate::ScopedPath) -> String {
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
            .map(|segment| segment.display_text())
            .collect::<Vec<_>>()
            .join(".")
    )
}

fn table_name(catalog: &Catalog, table: TableId) -> &str {
    catalog
        .tables
        .get(table.0)
        .map_or("<unknown>", |table| table.name.as_str())
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
    ) || matches!(expr, Expr::Variable(_));
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
    let name = &selection.name.target.name.text;
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
    let fragment_table = match catalog.resolve_table_ref_for(on) {
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

fn check_output_key_lengths(selections: &[Selection], errors: &mut Vec<CheckError>) {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            continue;
        }
        let key = response_key(selection);
        let bytes = key.len();
        if bytes > POSTGRES_RESULT_ALIAS_MAX_BYTES {
            let range = selection
                .alias
                .as_ref()
                .map_or(selection.name.range, |alias| alias.range);
            errors.push(CheckError {
                range,
                kind: CheckErrorKind::OutputKeyTooLong {
                    key,
                    bytes,
                    max: POSTGRES_RESULT_ALIAS_MAX_BYTES,
                },
            });
        }
    }
}

fn response_key(selection: &Selection) -> String {
    selection.alias.as_ref().map_or_else(
        || selection.name.output_name().to_string(),
        |alias| alias.text.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DsqlDiagnostic;
    use crate::syntax::{Diagnostic, DiagnosticCode, parse_source};

    fn diagnostics(source: &str) -> Vec<Diagnostic> {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        check_file(&parsed.source_file)
            .diagnostics
            .iter()
            .map(DsqlDiagnostic::to_transport)
            .collect()
    }

    #[test]
    fn hardcoded_catalog_accepts_columns_and_relations() {
        let diagnostics = diagnostics(
            "query Q { public::users { id name posts { title users { name } } } posts { users { email } } }",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn hardcoded_catalog_reports_unknown_table_and_field() {
        let diagnostics = diagnostics("query Q { comments { id } public::users { missing } }");
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
            diagnostics("query Q { public::users { id { name } name(where .id == 1) posts } }");
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
        let diagnostics =
            diagnostics("fragment UserFields on public::users { id posts { title } }");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn fragment_spreads_are_checked_in_query_context() {
        let diagnostics = diagnostics(
            "fragment UserFields on public::users { id posts { title } }\nquery Q { users { ...UserFields } }",
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
            "query Q { public::users { id public::posts { title } } public::posts { public::users { email } } }",
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn relation_clauses_are_checked_against_related_table() {
        let diagnostics = diagnostics(
            "query Q { public::users { posts(where .title == \"hello\" order by title) { id } } }",
        );

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn hardcoded_catalog_accepts_qualified_fragment_type() {
        let diagnostics =
            diagnostics("fragment UserFields on public::users { id public::posts { title } }");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn unqualified_table_names_default_to_public() {
        let diagnostics = diagnostics("query Q { users { id } }");

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn duplicate_output_keys_require_aliases() {
        let diagnostics =
            diagnostics("query Q { public::users { id } other_schema::users { id } }");

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
            "query Q { public_users: public::users { id } other_users: other_schema::users { id } }",
        );

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn output_keys_must_fit_postgres_result_alias_limit() {
        let diagnostics = diagnostics(
            "query Q { this_alias_name_is_far_longer_than_postgresql_allows_for_identifiers_and_should_shrink: users { id } }",
        );

        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, DiagnosticCode::OutputKeyTooLong);
        assert_eq!(
            diagnostics[0].message,
            "selection output key `this_alias_name_is_far_longer_than_postgresql_allows_for_identifiers_and_should_shrink` is 86 bytes; PostgreSQL result aliases must be at most 63 bytes"
        );
    }

    #[test]
    fn hardcoded_catalog_reports_fragment_field_errors() {
        let diagnostics = diagnostics("fragment UserFields on public::users { missing }");
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
