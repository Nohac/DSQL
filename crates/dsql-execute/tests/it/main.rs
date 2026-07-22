use dsql_execute::{ExecuteError, ExecutionBindings, materialize};
use dsql_metadata::{
    InputDefault, InputField, OperationMetadata, ResultShape, SqlMetadata, SqlParameterMetadata,
    SqlVariantCaseMetadata, SqlVariantMetadata,
};
use serde_json::Value;
use serde_json::json;

fn field(path: &str, data_type: &str, collection: bool, enum_values: &[&str]) -> InputField {
    InputField {
        path: path.to_string(),
        data_type: data_type.to_string(),
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
        kind: "query".to_string(),
        sql: SqlMetadata {
            dialect: "postgres".to_string(),
            text: "select $1, $2, $3 order by recorded_at {{params.direction}}".to_string(),
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
        kind: "query".to_string(),
        sql: SqlMetadata {
            dialect: "postgres".to_string(),
            text: "select $1".to_string(),
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

#[test]
fn shared_default_conformance_cases_match() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../tests/conformance/input-defaults.json"
    ))
    .expect("conformance fixture parses");
    let cases = cases.as_array().expect("conformance fixture is an array");

    for case in cases {
        let name = case["name"].as_str().expect("case has a name");
        let field: InputField =
            facet_json::from_str(&case["field"].to_string()).expect("case field metadata parses");
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
            let expected = case["error"].as_str().expect("case has an error");
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

    for variables in [json!({"params": null}), json!({"params": 5})] {
        let error = materialize(
            &operation_with_fields(vec![defaulted.clone()], &["params.nested.value"]),
            &ExecutionBindings {
                variables,
                context: json!({}),
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
        variables: json!({}),
        context: json!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
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
        variables: json!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "ascending"
            }
        }),
        context: json!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
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
        variables: json!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "descending"
            },
            "input": {"readings": {"ids": [2, 5, 8]}}
        }),
        context: json!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };

    insta::assert_debug_snapshot!(materialize(&operation(), &bindings).expect("materializes"));
}

#[test]
fn materialization_rejects_missing_and_unknown_variant_values() {
    let missing = materialize(&operation(), &ExecutionBindings::default())
        .expect_err("first required declaration is absent");
    assert!(matches!(missing, ExecuteError::MissingInput(path) if path == "params.station"));

    let bindings = ExecutionBindings {
        variables: json!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "sideways"
            },
            "input": {"readings": {"ids": [2]}}
        }),
        context: json!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };
    let invalid = materialize(&operation(), &bindings).expect_err("variant is closed");
    assert!(matches!(
        invalid,
        ExecuteError::InvalidVariant { path, value }
            if path == "params.direction" && value == "sideways"
    ));

    let bindings = ExecutionBindings {
        variables: json!({
            "params": {
                "station": null,
                "direction": "ascending"
            },
            "input": {"readings": {"ids": [2]}}
        }),
        context: json!({"tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a322"}),
    };
    let null = materialize(&operation(), &bindings).expect_err("required input is non-null");
    assert!(matches!(
        null,
        ExecuteError::InvalidInput { path, expected }
            if path == "params.station" && expected == "a non-null uuid"
    ));
}

#[test]
fn materialization_rejects_non_query_and_non_postgres_metadata() {
    let mut unsupported = operation();
    unsupported.kind = "mutation".to_string();
    assert!(matches!(
        materialize(&unsupported, &ExecutionBindings::default()),
        Err(ExecuteError::UnsupportedOperationKind(kind)) if kind == "mutation"
    ));

    unsupported.kind = "query".to_string();
    unsupported.sql.dialect = "sqlite".to_string();
    assert!(matches!(
        materialize(&unsupported, &ExecutionBindings::default()),
        Err(ExecuteError::UnsupportedDialect(dialect)) if dialect == "sqlite"
    ));
}
