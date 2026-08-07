//! Human-readable operation contracts and dry-run output.

use std::collections::BTreeMap;

use dsql_execute::{BoundParameter, MaterializedOperation, execution_sql};
use dsql_metadata::{
    ClosedValueSetMetadata, DynamicInputMetadata, InputDefault, InputField, OperationMetadata,
    ResultField, ResultFieldKind, ResultValueShape,
};

pub(crate) fn render_contract(
    scope: &str,
    operation: &OperationMetadata,
) -> Result<String, String> {
    let mut output = format!("operation {scope}::{}\n", operation.name);
    render_input_section(
        &mut output,
        "params",
        "params.",
        &operation.params,
        &operation.dynamic_inputs,
    )?;
    render_input_section(
        &mut output,
        "input",
        "input.",
        &operation.input,
        &operation.dynamic_inputs,
    )?;
    render_input_section(&mut output, "context", "context.", &operation.context, &[])?;
    output.push('\n');
    output.push_str("result\n");
    output.push_str(&result_object_type(&operation.result.fields, "", 0)?);
    output.push('\n');
    Ok(output)
}

pub(crate) fn render_dry_run(
    scope: &str,
    operation: &OperationMetadata,
    materialized: &MaterializedOperation,
) -> String {
    let mut output = format!(
        "operation {scope}::{}\n\nSQL\n{}\n\nparameters\n",
        operation.name,
        execution_sql(materialized),
    );
    if materialized.parameters.is_empty() {
        output.push_str("  <none>\n");
    } else {
        for (index, parameter) in materialized.parameters.iter().enumerate() {
            output.push_str(&format!(
                "  ${} {}: {}\n",
                index + 1,
                parameter.path,
                parameter_type(operation, parameter),
            ));
        }
    }
    output
}

fn render_input_section(
    output: &mut String,
    title: &str,
    prefix: &str,
    fields: &[InputField],
    dynamic_inputs: &[DynamicInputMetadata],
) -> Result<(), String> {
    output.push('\n');
    output.push_str(title);
    output.push('\n');
    if fields.is_empty() {
        output.push_str("  <none>\n");
        return Ok(());
    }

    let dynamic_by_path = dynamic_inputs
        .iter()
        .map(|input| (input.path.as_str(), input))
        .collect::<BTreeMap<_, _>>();
    for field in fields {
        let path = field.path.strip_prefix(prefix).ok_or_else(|| {
            format!(
                "operation input path `{}` does not belong to the `{title}` namespace",
                field.path
            )
        })?;
        let nullable = if field.nullable { "?" } else { "" };
        let data_type = match dynamic_by_path.get(field.path.as_str()) {
            Some(dynamic) => format!("{}({})", dynamic.kind, dynamic.surface),
            None => input_type(field)?,
        };
        output.push_str(&format!("  {path}{nullable}: {data_type}"));
        if let Some(default) = &field.default {
            output.push_str(" = ");
            output.push_str(&render_default(default)?);
        } else if !field.required {
            return Err(format!(
                "optional input `{}` has no compiler-provided default",
                field.path
            ));
        }
        output.push('\n');

        if let Some(dynamic) = dynamic_by_path.get(field.path.as_str()) {
            for candidate in &dynamic.fields {
                output.push_str(&format!(
                    "    {}: {}",
                    candidate.key,
                    scalar_type(
                        &candidate.data_type,
                        &candidate.closed_values,
                        candidate.nullable,
                    )?,
                ));
                let mut capabilities = Vec::new();
                if !candidate.operators.is_empty() {
                    capabilities.push(format!("operators {}", candidate.operators.join(", ")));
                }
                if !candidate.directions.is_empty() {
                    capabilities.push(format!("directions {}", candidate.directions.join(", ")));
                }
                if !capabilities.is_empty() {
                    output.push_str(&format!(" [{}]", capabilities.join("; ")));
                }
                output.push('\n');
            }
        }
    }
    Ok(())
}

fn input_type(field: &InputField) -> Result<String, String> {
    let scalar = scalar_type(&field.data_type, &field.closed_values, false)?;
    if field.collection == Some(true) {
        Ok(format!("Array<{scalar} | null>"))
    } else {
        Ok(scalar)
    }
}

fn scalar_type(
    data_type: &str,
    closed_values: &ClosedValueSetMetadata,
    nullable: bool,
) -> Result<String, String> {
    let mut rendered = if closed_values.values.is_empty() {
        data_type.to_string()
    } else {
        closed_values
            .values
            .iter()
            .map(|value| quote(&value.value))
            .collect::<Result<Vec<_>, _>>()?
            .join(" | ")
    };
    if nullable {
        rendered.push_str(" | null");
    }
    Ok(rendered)
}

fn render_default(default: &InputDefault) -> Result<String, String> {
    match default.kind.as_str() {
        "string" => default
            .value
            .as_deref()
            .ok_or_else(|| "string default is missing its value".to_string())
            .and_then(quote),
        "number" => default
            .value
            .clone()
            .ok_or_else(|| "number default is missing its value".to_string()),
        "boolean" => default
            .boolean
            .map(|value| value.to_string())
            .ok_or_else(|| "boolean default is missing its value".to_string()),
        "null" => Ok("null".to_string()),
        "empty_object" => Ok("{}".to_string()),
        "collection" => default
            .items
            .as_deref()
            .ok_or_else(|| "collection default is missing its items".to_string())
            .and_then(|items| {
                items
                    .iter()
                    .map(render_default)
                    .collect::<Result<Vec<_>, _>>()
            })
            .map(|items| format!("[{}]", items.join(", "))),
        kind => Err(format!("unknown input default kind `{kind}`")),
    }
}

fn result_object_type(
    fields: &[ResultField],
    parent_path: &str,
    indent: usize,
) -> Result<String, String> {
    let children = fields
        .iter()
        .filter(|field| field.parent_path == parent_path)
        .collect::<Vec<_>>();
    if children.is_empty() {
        return Ok("Record<string, never>".to_string());
    }
    let mut rendered = String::from("{\n");
    for field in children {
        rendered.push_str(&" ".repeat(indent + 2));
        rendered.push_str(&field.name);
        rendered.push_str(": ");
        rendered.push_str(&result_field_type(fields, field, indent + 2)?);
        rendered.push('\n');
    }
    rendered.push_str(&" ".repeat(indent));
    rendered.push('}');
    Ok(rendered)
}

fn result_field_type(
    fields: &[ResultField],
    field: &ResultField,
    indent: usize,
) -> Result<String, String> {
    let mut rendered = match field.kind {
        ResultFieldKind::Scalar => {
            let scalar = scalar_type(
                &field.value_type.name,
                &field.value_type.closed_values,
                false,
            )?;
            match field.value_type.shape {
                ResultValueShape::Scalar => scalar,
                ResultValueShape::DatabaseArray => format!("database_array<{scalar}>"),
                ResultValueShape::Object => {
                    return Err(format!(
                        "scalar result field `{}` has object value shape",
                        field.path
                    ));
                }
            }
        }
        ResultFieldKind::Object => result_object_type(fields, &field.path, indent)?,
        ResultFieldKind::Array => {
            format!(
                "Array<{}>",
                result_object_type(fields, &field.path, indent)?
            )
        }
    };
    if field.nullable {
        rendered.push_str(" | null");
    }
    Ok(rendered)
}

fn parameter_type(operation: &OperationMetadata, parameter: &BoundParameter) -> String {
    let declared = operation
        .params
        .iter()
        .chain(&operation.input)
        .chain(&operation.context)
        .find(|field| field.path == parameter.path);
    let collection = declared
        .and_then(|field| field.collection)
        .unwrap_or(parameter.collection);
    let mut rendered = if collection {
        if declared.is_some() {
            format!("Array<{} | null>", parameter.data_type)
        } else {
            format!("Array<{}>", parameter.data_type)
        }
    } else {
        parameter.data_type.clone()
    };
    if declared.is_some_and(|field| field.nullable) {
        rendered.push_str(" | null");
    }
    rendered
}

fn quote(value: &str) -> Result<String, String> {
    facet_json::to_string(&value.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use dsql_metadata::{
        ClosedValueSetMetadata, InputField, InputValidationMetadata, ResultField, ResultFieldKind,
        ResultValueShape, ResultValueTypeMetadata, WireEncoding, WireMetadata,
    };

    use super::{render_input_section, result_object_type};

    fn open_values() -> ClosedValueSetMetadata {
        ClosedValueSetMetadata {
            description: None,
            values: Vec::new(),
        }
    }

    fn text_wire() -> WireMetadata {
        WireMetadata {
            encoding: WireEncoding::Text,
            provider_type: None,
        }
    }

    #[test]
    fn result_contract_preserves_database_array_shape() {
        let fields = vec![ResultField {
            path: "values".to_string(),
            name: "values".to_string(),
            parent_path: String::new(),
            kind: ResultFieldKind::Scalar,
            value_type: ResultValueTypeMetadata {
                shape: ResultValueShape::DatabaseArray,
                name: "bigint".to_string(),
                display: Some("bigint[]".to_string()),
                wire: WireMetadata {
                    encoding: WireEncoding::BigInteger,
                    provider_type: None,
                },
                closed_values: open_values(),
            },
            nullable: true,
            access: "unconditional".to_string(),
        }];

        insta::assert_snapshot!(
            result_object_type(&fields, "", 0).expect("database-array contract renders")
        );
    }

    #[test]
    fn optional_inputs_without_defaults_are_rejected() {
        let fields = vec![InputField {
            path: "params.value".to_string(),
            data_type: "text".to_string(),
            wire: text_wire(),
            validation: InputValidationMetadata { pattern: None },
            collection: None,
            closed_values: open_values(),
            required: false,
            nullable: false,
            default: None,
        }];
        let mut output = String::new();
        let error = render_input_section(&mut output, "params", "params.", &fields, &[])
            .expect_err("optional fields require compiler-provided defaults");
        insta::assert_snapshot!(error);
    }
}
