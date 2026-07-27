use dsql_execute::{
    BoundParameter, ExecuteError, ExecutionBindings, MaterializedOperation, PostgresExecutor,
    materialize,
};
use dsql_metadata::{
    DefinitionKind, DynamicInputField, DynamicInputMetadata, DynamicInputSite,
    DynamicInputSiteField, DynamicPredicateOperatorMetadata, InputDefault, InputField,
    InputValidationMetadata, OperationMetadata, ResultShape, SqlDialect, SqlMetadata,
    SqlParameterMetadata, SqlVariantCaseMetadata, SqlVariantMetadata, WireEncoding, WireMetadata,
};
use facet_value::{Value, value};

fn wire(data_type: &str) -> WireMetadata {
    WireMetadata {
        encoding: match data_type {
            "uuid" => WireEncoding::Uuid,
            "text" => WireEncoding::Text,
            "timestamptz" => WireEncoding::Timestamptz,
            "int" => WireEncoding::Integer,
            "bigint" => WireEncoding::BigInteger,
            "numeric" => WireEncoding::Numeric,
            "float" => WireEncoding::Float,
            "boolean" => WireEncoding::Boolean,
            "json" => WireEncoding::Json,
            "dynamic_predicate" | "dynamic_order" => WireEncoding::Unsupported,
            _ => WireEncoding::Unsupported,
        },
        provider_type: None,
    }
}

fn field(path: &str, data_type: &str, collection: bool, enum_values: &[&str]) -> InputField {
    InputField {
        path: path.to_string(),
        data_type: data_type.to_string(),
        wire: wire(data_type),
        validation: InputValidationMetadata { pattern: None },
        collection: collection.then_some(true),
        enum_values: enum_values
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        required: true,
        nullable: false,
        default: None,
    }
}

fn operation() -> OperationMetadata {
    OperationMetadata {
        name: "StationReadings".to_string(),
        kind: DefinitionKind::Query,
        sql: SqlMetadata {
            dialect: SqlDialect::Postgres,
            text: "select $1, $2, $3 order by recorded_at {{params.direction}}".to_string(),
            compact_text: "select $1, $2, $3 order by recorded_at {{params.direction}}".to_string(),
            parameters: ["params.station", "input.readings.ids", "context.tenant_id"]
                .map(|path| SqlParameterMetadata {
                    path: path.to_string(),
                })
                .to_vec(),
            variants: vec![SqlVariantMetadata {
                path: "params.direction".to_string(),
                cases: vec![
                    SqlVariantCaseMetadata {
                        value: "ascending".to_string(),
                        text: "asc".to_string(),
                    },
                    SqlVariantCaseMetadata {
                        value: "descending".to_string(),
                        text: "desc".to_string(),
                    },
                ],
                null_text: None,
            }],
        },
        result: ResultShape { fields: Vec::new() },
        params: vec![
            field("params.station", "uuid", false, &[]),
            field(
                "params.direction",
                "text",
                false,
                &["ascending", "descending"],
            ),
        ],
        input: vec![field("input.readings.ids", "int", true, &[])],
        context: vec![field("context.tenant_id", "uuid", false, &[])],
        dynamic_inputs: Vec::new(),
        policies: Vec::new(),
        handoffs: Vec::new(),
        fragment_spreads: Vec::new(),
        source_map: Vec::new(),
    }
}

fn operation_with_fields(params: Vec<InputField>, parameters: &[&str]) -> OperationMetadata {
    OperationMetadata {
        name: "Inputs".to_string(),
        kind: DefinitionKind::Query,
        sql: SqlMetadata {
            dialect: SqlDialect::Postgres,
            text: "select $1".to_string(),
            compact_text: "select $1".to_string(),
            parameters: parameters
                .iter()
                .map(|path| SqlParameterMetadata {
                    path: (*path).to_string(),
                })
                .collect(),
            variants: Vec::new(),
        },
        result: ResultShape { fields: Vec::new() },
        params,
        input: Vec::new(),
        context: Vec::new(),
        dynamic_inputs: Vec::new(),
        policies: Vec::new(),
        handoffs: Vec::new(),
        fragment_spreads: Vec::new(),
        source_map: Vec::new(),
    }
}

fn dynamic_operation() -> OperationMetadata {
    let predicate_fields = ["u", "u2"]
        .into_iter()
        .map(|alias| DynamicInputSiteField {
            key: "id".to_string(),
            operators: vec![
                DynamicPredicateOperatorMetadata {
                    name: "in".to_string(),
                    value_kind: "collection".to_string(),
                    before_value: Some(format!("{alias}.id = ANY(")),
                    after_value: Some(")".to_string()),
                    cases: Vec::new(),
                },
                DynamicPredicateOperatorMetadata {
                    name: "is_null".to_string(),
                    value_kind: "boolean".to_string(),
                    before_value: None,
                    after_value: None,
                    cases: vec![
                        SqlVariantCaseMetadata {
                            value: "true".to_string(),
                            text: format!("{alias}.id IS NULL"),
                        },
                        SqlVariantCaseMetadata {
                            value: "false".to_string(),
                            text: format!("{alias}.id IS NOT NULL"),
                        },
                    ],
                },
            ],
            directions: Vec::new(),
        })
        .collect::<Vec<_>>();
    let predicate_sites = ["{{dynamic:0}}", "{{dynamic:1}}"]
        .into_iter()
        .zip(predicate_fields)
        .map(|(marker, id)| DynamicInputSite {
            marker: marker.to_string(),
            identity_sql: "TRUE".to_string(),
            fields: vec![
                id,
                DynamicInputSiteField {
                    key: "name".to_string(),
                    operators: vec![DynamicPredicateOperatorMetadata {
                        name: "like".to_string(),
                        value_kind: "scalar".to_string(),
                        before_value: Some(if marker.contains(":0") {
                            "u.name LIKE ".to_string()
                        } else {
                            "u2.name LIKE ".to_string()
                        }),
                        after_value: None,
                        cases: Vec::new(),
                    }],
                    directions: Vec::new(),
                },
            ],
        })
        .collect();
    OperationMetadata {
        name: "Dynamic".to_string(),
        kind: DefinitionKind::Query,
        sql: SqlMetadata {
            dialect: SqlDialect::Postgres,
            text: "select $1 where {{dynamic:0}} and {{dynamic:1}} order by {{dynamic:2}}"
                .to_string(),
            compact_text: "select $1 where {{dynamic:0}} and {{dynamic:1}} order by {{dynamic:2}}"
                .to_string(),
            parameters: vec![SqlParameterMetadata {
                path: "params.tenant".to_string(),
            }],
            variants: Vec::new(),
        },
        result: ResultShape { fields: Vec::new() },
        params: vec![
            field("params.tenant", "int", false, &[]),
            field("params.search", "dynamic_predicate", false, &[]),
            field("params.order", "dynamic_order", false, &[]),
        ],
        input: Vec::new(),
        context: Vec::new(),
        dynamic_inputs: vec![
            DynamicInputMetadata {
                path: "params.search".to_string(),
                kind: "predicate".to_string(),
                surface: "selected".to_string(),
                fields: vec![
                    DynamicInputField {
                        key: "id".to_string(),
                        catalog_path: "public.users.id".to_string(),
                        data_type: "int".to_string(),
                        wire: wire("int"),
                        validation: InputValidationMetadata { pattern: None },
                        nullable: false,
                        access: "unconditional".to_string(),
                        operators: vec!["in".to_string(), "is_null".to_string()],
                        directions: Vec::new(),
                    },
                    DynamicInputField {
                        key: "name".to_string(),
                        catalog_path: "public.users.name".to_string(),
                        data_type: "text".to_string(),
                        wire: wire("text"),
                        validation: InputValidationMetadata { pattern: None },
                        nullable: false,
                        access: "unconditional".to_string(),
                        operators: vec!["like".to_string()],
                        directions: Vec::new(),
                    },
                ],
                sites: predicate_sites,
            },
            DynamicInputMetadata {
                path: "params.order".to_string(),
                kind: "order".to_string(),
                surface: "selected".to_string(),
                fields: vec![DynamicInputField {
                    key: "name".to_string(),
                    catalog_path: "public.users.name".to_string(),
                    data_type: "text".to_string(),
                    wire: wire("text"),
                    validation: InputValidationMetadata { pattern: None },
                    nullable: false,
                    access: "unconditional".to_string(),
                    operators: Vec::new(),
                    directions: vec!["desc_nulls_last".to_string()],
                }],
                sites: vec![DynamicInputSite {
                    marker: "{{dynamic:2}}".to_string(),
                    identity_sql: "NULL::integer".to_string(),
                    fields: vec![DynamicInputSiteField {
                        key: "name".to_string(),
                        operators: Vec::new(),
                        directions: vec![SqlVariantCaseMetadata {
                            value: "desc_nulls_last".to_string(),
                            text: "u.name DESC NULLS LAST".to_string(),
                        }],
                    }],
                }],
            },
        ],
        policies: Vec::new(),
        handoffs: Vec::new(),
        fragment_spreads: Vec::new(),
        source_map: Vec::new(),
    }
}

#[test]
fn input_metadata_without_wire_encoding_is_rejected() {
    let error = facet_json::from_str::<InputField>(
        r#"{
          "path": "params.value",
          "data_type": "text",
          "enum_values": [],
          "required": true,
          "nullable": false
        }"#,
    )
    .expect_err("version 3 input fields require wire metadata");

    insta::assert_snapshot!(error.to_string());
}

#[test]
fn shared_default_conformance_cases_match() {
    let cases: Value = facet_json::from_str(include_str!(
        "../../../../tests/conformance/input-defaults.json"
    ))
    .expect("conformance fixture parses");
    let cases = cases.as_array().expect("conformance fixture is an array");

    for case in cases {
        let case = case.as_object().expect("conformance case is an object");
        let name = case
            .get("name")
            .and_then(Value::as_string)
            .map(|value| value.as_str())
            .expect("case has a name");
        let field = case.get("field").expect("case has field metadata").clone();
        let field: InputField = facet_value::from_value(field).expect("case field metadata parses");
        let result = materialize(
            &operation_with_fields(vec![field], &["params.value"]),
            &ExecutionBindings::default(),
        );

        if let Some(expected) = case.get("expected") {
            assert!(result.is_ok(), "{name} should materialize: {result:?}");
            if let Ok(materialized) = result {
                assert_eq!(materialized.parameters[0].value, *expected, "{name}");
            }
        } else {
            let expected = case
                .get("error")
                .and_then(Value::as_string)
                .map(|value| value.as_str())
                .expect("case has an error");
            let error = result.expect_err("case should fail").to_string();
            assert!(error.contains(expected), "{name}: {error}");
        }
    }
}

#[test]
fn materialization_rejects_invalid_envelopes_and_unused_required_fields() {
    let mut defaulted = field("params.nested.value", "int", false, &[]);
    defaulted.required = false;
    defaulted.default = Some(InputDefault {
        kind: "number".to_string(),
        value: Some("7".to_string()),
        boolean: None,
        items: None,
    });

    for variables in [value!({"params": null}), value!({"params": 5})] {
        let error = materialize(
            &operation_with_fields(vec![defaulted.clone()], &["params.nested.value"]),
            &ExecutionBindings {
                variables,
                context: value!({}),
            },
        )
        .expect_err("explicit invalid envelope is rejected");
        assert!(
            matches!(error, ExecuteError::InvalidInput { path, .. } if path == "params.nested.value")
        );
    }

    let error = materialize(
        &operation_with_fields(vec![field("params.unused", "int", false, &[])], &[]),
        &ExecutionBindings::default(),
    )
    .expect_err("unused required input is still validated");
    assert!(matches!(error, ExecuteError::MissingInput(path) if path == "params.unused"));

    let error = materialize(
        &operation_with_fields(Vec::new(), &["params.undeclared"]),
        &ExecutionBindings::default(),
    )
    .expect_err("SQL parameters require declared input metadata");
    assert!(matches!(
        error,
        ExecuteError::UndeclaredParameter(path) if path == "params.undeclared"
    ));
}

#[test]
fn shared_input_value_conformance_cases_match() {
    let cases: Value = facet_json::from_str(include_str!(
        "../../../../tests/conformance/input-values.json"
    ))
    .expect("conformance fixture parses");
    let cases = cases.as_array().expect("conformance fixture is an array");

    for case in cases {
        let case = case.as_object().expect("conformance case is an object");
        let name = case
            .get("name")
            .and_then(Value::as_string)
            .map(|value| value.as_str())
            .expect("case has a name");
        let field = case.get("field").expect("case has field metadata").clone();
        let field: InputField = facet_value::from_value(field).expect("case field metadata parses");
        let variables = case
            .get("bindings")
            .expect("case has a bindings envelope")
            .clone();
        let result = materialize(
            &operation_with_fields(vec![field], &["params.value"]),
            &ExecutionBindings {
                variables,
                context: value!({}),
            },
        );

        if let Some(expected) = case.get("expected") {
            assert!(result.is_ok(), "{name} should materialize: {result:?}");
            if let Ok(materialized) = result {
                assert_eq!(materialized.parameters[0].value, *expected, "{name}");
            }
        } else {
            let expected = case
                .get("error")
                .and_then(Value::as_string)
                .map(|value| value.as_str())
                .expect("case has an error");
            let error = result.expect_err("case should fail").to_string();
            assert!(error.contains(expected), "{name}: {error}");
        }
    }
}

#[test]
fn shared_dynamic_input_conformance_cases_match() {
    let fixture: Value = facet_json::from_str(include_str!(
        "../../../../tests/conformance/dynamic-inputs.json"
    ))
    .expect("dynamic conformance fixture parses");
    let fixture = fixture
        .as_object()
        .expect("dynamic conformance fixture is an object");
    let operation: OperationMetadata = facet_value::from_value(
        fixture
            .get("operation")
            .expect("fixture has operation metadata")
            .clone(),
    )
    .expect("fixture operation metadata parses");
    let cases = fixture
        .get("cases")
        .and_then(Value::as_array)
        .expect("fixture has conformance cases");

    for case in cases {
        let case = case.as_object().expect("conformance case is an object");
        let name = case
            .get("name")
            .and_then(Value::as_string)
            .map(|value| value.as_str())
            .expect("case has a name");
        let variables = case.get("bindings").expect("case has bindings").clone();
        let result = materialize(
            &operation,
            &ExecutionBindings {
                variables,
                context: value!({}),
            },
        );

        if let Some(expected_sql) = case.get("expected_sql") {
            let expected_sql = expected_sql
                .as_string()
                .map(|value| value.as_str())
                .expect("expected SQL is a string");
            let expected_values = case
                .get("expected_values")
                .and_then(Value::as_array)
                .expect("successful case has expected values");
            let materialized = result.expect("dynamic conformance case materializes");
            assert_eq!(materialized.sql, expected_sql, "{name}");
            let values = materialized
                .parameters
                .into_iter()
                .map(|parameter| parameter.value)
                .collect::<Vec<_>>();
            assert_eq!(values, expected_values.as_slice(), "{name}");
        } else {
            let expected = case
                .get("error")
                .and_then(Value::as_string)
                .map(|value| value.as_str())
                .expect("rejection case has an error");
            let error = result.expect_err("dynamic conformance case rejects");
            assert!(error.to_string().contains(expected), "{name}: {error}");
        }
    }
}

#[test]
fn trusted_context_is_never_defaulted() {
    let mut operation = operation_with_fields(Vec::new(), &[]);
    let mut context = field("context.tenant_id", "uuid", false, &[]);
    context.required = false;
    context.default = Some(InputDefault {
        kind: "string".to_string(),
        value: Some("018f6f19-795f-7c3d-b1b3-8f177ab8a321".to_string()),
        boolean: None,
        items: None,
    });
    operation.context.push(context);

    let error = materialize(&operation, &ExecutionBindings::default())
        .expect_err("trusted context cannot receive a source default");
    assert!(matches!(error, ExecuteError::MissingInput(path) if path == "context.tenant_id"));
}

#[test]
fn materialization_applies_typed_defaults_and_nullable_variants() {
    let mut operation = operation();
    operation.params[0].required = false;
    operation.params[0].default = Some(InputDefault {
        kind: "string".to_string(),
        value: Some("018f6f19-795f-7c3d-b1b3-8f177ab8a321".to_string()),
        boolean: None,
        items: None,
    });
    operation.params[1].required = false;
    operation.params[1].nullable = true;
    operation.params[1].default = Some(InputDefault {
        kind: "null".to_string(),
        value: None,
        boolean: None,
        items: None,
    });
    operation.sql.variants[0].null_text = Some("null".to_string());
    operation.input[0].required = false;
    operation.input[0].default = Some(InputDefault {
        kind: "collection".to_string(),
        value: None,
        boolean: None,
        items: Some(vec![InputDefault {
            kind: "number".to_string(),
            value: Some("7".to_string()),
            boolean: None,
            items: None,
        }]),
    });
    let bindings = ExecutionBindings {
        variables: value!({}),
        context: value!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };

    insta::assert_debug_snapshot!(materialize(&operation, &bindings).expect("materializes"));
}

#[test]
fn materialization_distinguishes_null_collection_and_collection_value_defaults() {
    let mut null_collection = operation();
    null_collection.input[0].required = false;
    null_collection.input[0].nullable = true;
    null_collection.input[0].default = Some(InputDefault {
        kind: "null".to_string(),
        value: None,
        boolean: None,
        items: None,
    });
    let bindings = ExecutionBindings {
        variables: value!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "ascending"
            }
        }),
        context: value!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };

    let mut collection_value = null_collection.clone();
    collection_value.input[0].nullable = false;
    collection_value.input[0].default = Some(InputDefault {
        kind: "collection".to_string(),
        value: None,
        boolean: None,
        items: Some(vec![InputDefault {
            kind: "number".to_string(),
            value: Some("7".to_string()),
            boolean: None,
            items: None,
        }]),
    });

    insta::assert_debug_snapshot!(
        "null_collection",
        materialize(&null_collection, &bindings).expect("null collection materializes")
    );
    insta::assert_debug_snapshot!(
        "collection_value",
        materialize(&collection_value, &bindings).expect("collection value materializes")
    );
}

#[test]
fn materialization_resolves_variants_and_all_input_namespaces() {
    let bindings = ExecutionBindings {
        variables: value!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "descending"
            },
            "input": {"readings": {"ids": [2, 5, 8]}}
        }),
        context: value!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };

    insta::assert_debug_snapshot!(materialize(&operation(), &bindings).expect("materializes"));
}

#[test]
fn materialization_lowers_bounded_dynamic_inputs_and_reuses_operands() {
    let bindings = ExecutionBindings {
        variables: value!({
            "params": {
                "tenant": 7,
                "search": {
                    "and": [{"name": {"like": "A%"}}, {}],
                    "or": [
                        {"id": {"in": [1, 2]}},
                        {"not": {"id": {"is_null": true}}}
                    ]
                },
                "order": [{"name": "desc_nulls_last"}]
            }
        }),
        context: value!({}),
    };

    insta::assert_debug_snapshot!(
        materialize(&dynamic_operation(), &bindings).expect("dynamic operation materializes")
    );
}

#[test]
fn materialization_uses_dynamic_identities_and_rejects_unknown_capabilities() {
    let empty = ExecutionBindings {
        variables: value!({
            "params": {"tenant": 7, "search": {}, "order": []}
        }),
        context: value!({}),
    };
    insta::assert_debug_snapshot!(
        "identity",
        materialize(&dynamic_operation(), &empty).expect("empty dynamic inputs materialize")
    );

    let unknown = ExecutionBindings {
        variables: value!({
            "params": {
                "tenant": 7,
                "search": {"secret": {"eq": "nope"}},
                "order": []
            }
        }),
        context: value!({}),
    };
    let error = materialize(&dynamic_operation(), &unknown)
        .expect_err("unknown dynamic fields are rejected");
    assert!(
        matches!(error, ExecuteError::InvalidInput { path, .. } if path == "params.search.secret")
    );
}

#[test]
fn materialization_defaults_and_nullable_dynamic_inputs_to_identities() {
    let mut operation = dynamic_operation();
    for field in &mut operation.params {
        match field.data_type.as_str() {
            "dynamic_predicate" => {
                field.required = false;
                field.nullable = true;
                field.default = Some(InputDefault {
                    kind: "empty_object".to_string(),
                    value: None,
                    boolean: None,
                    items: None,
                });
            }
            "dynamic_order" => {
                field.required = false;
                field.nullable = true;
                field.default = Some(InputDefault {
                    kind: "collection".to_string(),
                    value: None,
                    boolean: None,
                    items: Some(Vec::new()),
                });
            }
            _ => {}
        }
    }
    let omitted = ExecutionBindings {
        variables: value!({"params": {"tenant": 7}}),
        context: value!({}),
    };
    let nullable = ExecutionBindings {
        variables: value!({
            "params": {"tenant": 7, "search": null, "order": null}
        }),
        context: value!({}),
    };

    let omitted = materialize(&operation, &omitted).expect("defaults materialize");
    let nullable = materialize(&operation, &nullable).expect("null identities materialize");
    assert_eq!(omitted, nullable);
    insta::assert_debug_snapshot!("defaulted_dynamic_identities", omitted);
}

#[test]
fn materialization_rejects_malformed_dynamic_inputs() {
    let cases = [
        (
            value!({
                "params": {
                    "tenant": 7,
                    "search": {"name": {"eq": "not allowed"}},
                    "order": []
                }
            }),
            "params.search.name.eq",
        ),
        (
            value!({
                "params": {
                    "tenant": 7,
                    "search": {"name": {"like": null}},
                    "order": []
                }
            }),
            "params.search.name.like",
        ),
        (
            value!({
                "params": {
                    "tenant": 7,
                    "search": {"id": {"in": [1, null]}},
                    "order": []
                }
            }),
            "params.search.id.in",
        ),
        (
            value!({
                "params": {
                    "tenant": 7,
                    "search": {"or": {}},
                    "order": []
                }
            }),
            "params.search.or",
        ),
        (
            value!({
                "params": {
                    "tenant": 7,
                    "search": {},
                    "order": [{"name": "desc_nulls_last", "id": "desc_nulls_last"}]
                }
            }),
            "params.order[0]",
        ),
        (
            value!({
                "params": {
                    "tenant": 7,
                    "search": {},
                    "order": [{"name": "sideways"}]
                }
            }),
            "params.order[0]",
        ),
    ];

    for (variables, expected_path) in cases {
        let error = materialize(
            &dynamic_operation(),
            &ExecutionBindings {
                variables,
                context: value!({}),
            },
        )
        .expect_err("malformed dynamic input is rejected");
        assert!(
            matches!(error, ExecuteError::InvalidInput { ref path, .. } if path == expected_path),
            "expected an error at {expected_path}, got {error}",
        );
    }
}

#[test]
fn materialization_rejects_missing_and_unknown_variant_values() {
    let missing = materialize(&operation(), &ExecutionBindings::default())
        .expect_err("first required declaration is absent");
    assert!(matches!(missing, ExecuteError::MissingInput(path) if path == "params.station"));

    let bindings = ExecutionBindings {
        variables: value!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "sideways"
            },
            "input": {"readings": {"ids": [2]}}
        }),
        context: value!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };
    let invalid = materialize(&operation(), &bindings).expect_err("variant is closed");
    assert!(matches!(
        invalid,
        ExecuteError::InvalidVariant { path, value }
            if path == "params.direction" && value == "sideways"
    ));

    let bindings = ExecutionBindings {
        variables: value!({
            "params": {
                "station": null,
                "direction": "ascending"
            },
            "input": {"readings": {"ids": [2]}}
        }),
        context: value!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };
    let null = materialize(&operation(), &bindings).expect_err("required input is non-null");
    assert!(matches!(
        null,
        ExecuteError::InvalidInput { path, expected }
            if path == "params.station" && expected == "a non-null uuid"
    ));
}

#[test]
fn materialization_rejects_non_query_metadata() {
    let mut unsupported = operation();
    unsupported.kind = DefinitionKind::Fragment;
    assert!(matches!(
        materialize(&unsupported, &ExecutionBindings::default()),
        Err(ExecuteError::UnsupportedOperationKind(kind)) if kind == "fragment"
    ));
}

#[tokio::test]
async fn postgres_json_bindings_roundtrip_through_sqlx() {
    let Ok(database_url) = std::env::var("DSQL_OBSERVATORY_DATABASE_URL") else {
        return;
    };
    let executor = PostgresExecutor::connect(&database_url)
        .await
        .expect("observatory connects");
    let materialized = MaterializedOperation {
        sql: "select $1::json as scalar, $2::jsonb[] as items, $3::jsonb as absent".to_string(),
        parameters: vec![
            BoundParameter {
                path: "params.scalar".to_string(),
                data_type: "json".to_string(),
                wire: WireEncoding::Json,
                provider_type: None,
                collection: false,
                value: value!({ "nested": [1, true] }),
            },
            BoundParameter {
                path: "params.items".to_string(),
                data_type: "json".to_string(),
                wire: WireEncoding::Json,
                provider_type: None,
                collection: true,
                value: value!([{ "item": 1 }, null, ["nested"]]),
            },
            BoundParameter {
                path: "params.absent".to_string(),
                data_type: "json".to_string(),
                wire: WireEncoding::Json,
                provider_type: None,
                collection: false,
                value: Value::NULL,
            },
        ],
    };

    let output = executor
        .execute_materialized(&materialized)
        .await
        .expect("JSON bindings execute");
    assert_eq!(
        output,
        value!({
            "scalar": { "nested": [1, true] },
            "items": [{ "item": 1 }, null, ["nested"]],
            "absent": null
        })
    );
}

#[tokio::test]
async fn postgres_text_cast_bindings_cover_provider_scalars_and_collections() {
    let Ok(database_url) = std::env::var("DSQL_OBSERVATORY_DATABASE_URL") else {
        return;
    };
    let executor = PostgresExecutor::connect(&database_url)
        .await
        .expect("observatory connects");
    let text_cast = |path: &str, name: &str, collection: bool, value: Value| BoundParameter {
        path: path.to_string(),
        data_type: "text".to_string(),
        wire: WireEncoding::TextCast,
        provider_type: Some(("pg_catalog".to_string(), name.to_string())),
        collection,
        value,
    };
    let materialized = MaterializedOperation {
        sql: "select (($1)::text)::date as event_date, \
              (($2)::text)::timestamp as local_time, \
              (($3)::text)::inet as address, \
              (($4)::text[])::date[] as event_dates"
            .to_string(),
        parameters: vec![
            text_cast("params.event_date", "date", false, value!("2026-07-27")),
            text_cast(
                "params.local_time",
                "timestamp",
                false,
                value!("2026-07-27 12:34:56"),
            ),
            text_cast("params.address", "inet", false, value!("127.0.0.1")),
            text_cast(
                "params.event_dates",
                "date",
                true,
                value!(["2026-07-27", "2026-07-28"]),
            ),
        ],
    };

    let output = executor
        .execute_materialized(&materialized)
        .await
        .expect("provider text casts execute");
    let object = output.as_object().expect("query returns one object");
    assert_eq!(
        object
            .get("event_date")
            .and_then(Value::as_string)
            .map(|value| value.as_str()),
        Some("2026-07-27")
    );
    assert_eq!(
        object
            .get("local_time")
            .and_then(Value::as_string)
            .map(|value| value.as_str()),
        Some("2026-07-27T12:34:56")
    );
    assert!(
        object
            .get("address")
            .and_then(Value::as_string)
            .is_some_and(|value| value.as_str().starts_with("127.0.0.1"))
    );
    let expected_dates = value!(["2026-07-27", "2026-07-28"]);
    assert_eq!(object.get("event_dates"), Some(&expected_dates));
}

#[tokio::test]
async fn postgres_text_cast_failures_have_a_stable_input_error() {
    let Ok(database_url) = std::env::var("DSQL_OBSERVATORY_DATABASE_URL") else {
        return;
    };
    let executor = PostgresExecutor::connect(&database_url)
        .await
        .expect("observatory connects");
    let materialized = MaterializedOperation {
        sql: "select (($1)::text)::date as event_date".to_string(),
        parameters: vec![BoundParameter {
            path: "params.event_date".to_string(),
            data_type: "text".to_string(),
            wire: WireEncoding::TextCast,
            provider_type: Some(("pg_catalog".to_string(), "date".to_string())),
            collection: false,
            value: value!("not-a-date"),
        }],
    };

    let error = executor
        .execute_materialized(&materialized)
        .await
        .expect_err("invalid provider input fails");
    assert!(matches!(
        error,
        ExecuteError::InvalidProviderInput { path, data_type }
            if path == "params.event_date" && data_type == "pg_catalog.date"
    ));
}

#[tokio::test]
async fn postgres_accepts_the_dynamic_order_identity_expression() {
    let Ok(database_url) = std::env::var("DSQL_OBSERVATORY_DATABASE_URL") else {
        return;
    };
    let executor = PostgresExecutor::connect(&database_url)
        .await
        .expect("observatory connects");
    let materialized = MaterializedOperation {
        sql: "select JSON_AGG(value order by NULL::integer) as values from (values (2), (1)) as source(value)".to_string(),
        parameters: Vec::new(),
    };

    let output = executor
        .execute_materialized(&materialized)
        .await
        .expect("typed constant dynamic order identity executes");
    assert!(
        output
            .as_object()
            .and_then(|object| object.get("values"))
            .is_some()
    );
}
