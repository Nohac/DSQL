use dsql_execute::{ExecuteError, ExecutionBindings, materialize};
use dsql_metadata::{
    InputDefault, InputField, OperationMetadata, ResultShape, SqlMetadata, SqlParameterMetadata,
    SqlVariantCaseMetadata, SqlVariantMetadata,
};
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
        .expect_err("required variant is absent");
    assert!(matches!(missing, ExecuteError::MissingInput(path) if path == "params.direction"));

    let bindings = ExecutionBindings {
        variables: json!({
            "params": {
                "station": "018f6f19-795f-7c3d-b1b3-8f177ab8a321",
                "direction": "sideways"
            }
        }),
        context: json!({}),
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
