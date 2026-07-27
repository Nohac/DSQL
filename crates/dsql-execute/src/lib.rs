//! Native PostgreSQL execution for compiled [`dsql_metadata::OperationMetadata`].

use std::collections::BTreeMap;
use std::str::FromStr;

use dsql_metadata::{
    DefinitionKind, DynamicInputField, DynamicInputMetadata, DynamicInputSite,
    DynamicInputSiteField, InputDefault, InputField, OperationMetadata, WireEncoding, WireMetadata,
};
use facet_value::{VArray, VObject, Value};
use sqlx::postgres::{PgArguments, PgPool, PgPoolOptions};
use sqlx::query::QueryScalar;
use sqlx::types::{
    BigDecimal, Json, JsonRawValue, Uuid,
    chrono::{DateTime, FixedOffset},
};
use sqlx::{AssertSqlSafe, Postgres};

const MIN_SAFE_INTEGER: i64 = -9_007_199_254_740_991;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Public inputs and trusted server context for one execution.
#[derive(Clone, Debug)]
pub struct ExecutionBindings {
    /// The metadata-shaped `params` and `input` trees.
    pub variables: Value,
    /// Context fields without the `context` wrapper.
    pub context: Value,
}

impl Default for ExecutionBindings {
    fn default() -> Self {
        Self {
            variables: VObject::new().into(),
            context: VObject::new().into(),
        }
    }
}

/// A reusable PostgreSQL operation executor.
#[derive(Clone, Debug)]
pub struct PostgresExecutor {
    pool: PgPool,
}

/// Validated SQL and its ordered values.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterializedOperation {
    pub sql: String,
    pub parameters: Vec<BoundParameter>,
}

/// One validated positional bind.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundParameter {
    pub path: String,
    pub data_type: String,
    pub wire: WireEncoding,
    pub provider_type: Option<(String, String)>,
    pub collection: bool,
    pub value: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("operation SQL parameter `{0}` has no declared input type")]
    UndeclaredParameter(String),
    #[error("required operation input `{0}` was not provided")]
    MissingInput(String),
    #[error("operation input `{path}` must be {expected}")]
    InvalidInput { path: String, expected: String },
    #[error("operation variant `{path}` does not accept `{value}`")]
    InvalidVariant { path: String, value: String },
    #[error("operation input `{path}` uses unsupported logical type `{data_type}`")]
    UnsupportedType { path: String, data_type: String },
    #[error("operation input `{path}` is not valid for provider type `{data_type}`")]
    InvalidProviderInput { path: String, data_type: String },
    #[error("operation dynamic input metadata is invalid: {0}")]
    InvalidDynamicMetadata(String),
    #[error("operation kind `{0}` cannot be executed as a PostgreSQL query")]
    UnsupportedOperationKind(String),
    #[error("operation returned invalid JSON: {0}")]
    InvalidOutput(String),
    #[error("operation JSON input `{path}` could not be encoded: {message}")]
    InvalidJsonParameter { path: String, message: String },
}

impl PostgresExecutor {
    pub async fn connect(database_url: &str) -> Result<Self, ExecuteError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn execute(
        &self,
        operation: &OperationMetadata,
        bindings: &ExecutionBindings,
    ) -> Result<Value, ExecuteError> {
        let materialized = materialize(operation, bindings)?;
        self.execute_materialized(&materialized).await
    }

    /// Executes an operation that has already passed input materialization.
    pub async fn execute_materialized(
        &self,
        materialized: &MaterializedOperation,
    ) -> Result<Value, ExecuteError> {
        let inner = materialized.sql.trim().trim_end_matches(';');
        let mut query = sqlx::query_scalar::<Postgres, Option<String>>(AssertSqlSafe(format!(
            "select ROW_TO_JSON(\"result\")::text from ({inner}) as \"result\""
        )));
        for parameter in &materialized.parameters {
            query = bind(query, parameter)?;
        }
        let output = match query.fetch_one(&self.pool).await {
            Ok(output) => output,
            Err(error) => {
                if database_error_is_class(&error, "22")
                    && let Some(invalid) = self
                        .probe_text_cast_parameters(&materialized.parameters)
                        .await
                {
                    return Err(invalid);
                }
                return Err(ExecuteError::Database(error));
            }
        };
        output.map_or(Ok(Value::NULL), |output| {
            let mut output: Value = facet_json::from_str(&output)
                .map_err(|error| ExecuteError::InvalidOutput(error.to_string()))?;
            sort_object_keys(&mut output);
            Ok(output)
        })
    }

    async fn probe_text_cast_parameters(
        &self,
        parameters: &[BoundParameter],
    ) -> Option<ExecuteError> {
        for parameter in parameters {
            if parameter.wire != WireEncoding::TextCast || parameter.value.is_null() {
                continue;
            }
            let Some((schema, name)) = &parameter.provider_type else {
                continue;
            };
            let type_name = format!(
                "\"{}\".\"{}\"",
                schema.replace('"', "\"\""),
                name.replace('"', "\"\"")
            );
            let sql = if parameter.collection {
                format!("select ((($1)::text[])::{type_name}[])::text")
            } else {
                format!("select ((($1)::text)::{type_name})::text")
            };
            let query = sqlx::query_scalar::<Postgres, Option<String>>(AssertSqlSafe(sql));
            let Ok(query) = bind(query, parameter) else {
                continue;
            };
            if let Err(error) = query.fetch_optional(&self.pool).await
                && database_error_is_class(&error, "22")
            {
                return Some(ExecuteError::InvalidProviderInput {
                    path: parameter.path.clone(),
                    data_type: format!("{schema}.{name}"),
                });
            }
        }
        None
    }
}

pub fn materialize(
    operation: &OperationMetadata,
    bindings: &ExecutionBindings,
) -> Result<MaterializedOperation, ExecuteError> {
    if operation.kind != DefinitionKind::Query {
        return Err(ExecuteError::UnsupportedOperationKind(
            operation.kind.as_ref().to_string(),
        ));
    }
    let declared_fields = operation
        .params
        .iter()
        .chain(&operation.input)
        .chain(&operation.context)
        .collect::<Vec<_>>();
    let fields = declared_fields
        .iter()
        .copied()
        .map(|field| (field.path.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut values = BTreeMap::new();
    for field in declared_fields {
        if let Some(value) = input_value(field, bindings)? {
            values.insert(field.path.as_str(), value);
        }
    }
    let mut sql = operation.sql.text.clone();

    for variant in &operation.sql.variants {
        declared(&fields, &variant.path)?;
        let value = materialized_value(&values, &variant.path)?;
        if value.is_null() {
            let Some(text) = &variant.null_text else {
                return Err(invalid(&variant.path, "a non-null string variant"));
            };
            sql = sql.replace(&format!("{{{{{}}}}}", variant.path), text);
            continue;
        }
        let Some(value) = string_value(value) else {
            return Err(invalid(&variant.path, "a string variant"));
        };
        let Some(case) = variant.cases.iter().find(|case| case.value == value) else {
            return Err(ExecuteError::InvalidVariant {
                path: variant.path.clone(),
                value: value.to_string(),
            });
        };
        sql = sql.replace(&format!("{{{{{}}}}}", variant.path), &case.text);
    }

    let mut parameters = operation
        .sql
        .parameters
        .iter()
        .map(|parameter| {
            let field = declared(&fields, &parameter.path)?;
            Ok(BoundParameter {
                path: parameter.path.clone(),
                data_type: field.data_type.clone(),
                wire: field_wire(field),
                provider_type: field_provider_type(field),
                collection: field.collection == Some(true),
                value: materialized_value(&values, &parameter.path)?.clone(),
            })
        })
        .collect::<Result<Vec<_>, ExecuteError>>()?;
    materialize_dynamic_inputs(
        &mut sql,
        &operation.dynamic_inputs,
        &values,
        &mut parameters,
    )?;

    Ok(MaterializedOperation { sql, parameters })
}

fn materialize_dynamic_inputs(
    sql: &mut String,
    inputs: &[DynamicInputMetadata],
    values: &BTreeMap<&str, Value>,
    parameters: &mut Vec<BoundParameter>,
) -> Result<(), ExecuteError> {
    for input in inputs {
        let value = materialized_value(values, &input.path)?;
        let value = if value.is_null() {
            match input.kind.as_str() {
                "predicate" => VObject::new().into(),
                "order" => VArray::new().into(),
                _ => {
                    return Err(ExecuteError::InvalidDynamicMetadata(format!(
                        "input `{}` has unknown kind `{}`",
                        input.path, input.kind
                    )));
                }
            }
        } else {
            value.clone()
        };
        let parameter_start = parameters.len();
        let mut expected_parameter_end = None;
        for site in &input.sites {
            let mut operand_index = parameter_start;
            let rendered = match input.kind.as_str() {
                "predicate" => render_dynamic_predicate(
                    input,
                    site,
                    &value,
                    &input.path,
                    parameters,
                    &mut operand_index,
                )?
                .unwrap_or_else(|| site.identity_sql.clone()),
                "order" => render_dynamic_order(input, site, &value, &input.path)?
                    .unwrap_or_else(|| site.identity_sql.clone()),
                kind => {
                    return Err(ExecuteError::InvalidDynamicMetadata(format!(
                        "input `{}` has unknown kind `{kind}`",
                        input.path
                    )));
                }
            };
            if let Some(expected) = expected_parameter_end {
                if operand_index != expected {
                    return Err(ExecuteError::InvalidDynamicMetadata(format!(
                        "sites for `{}` do not consume the same operands",
                        input.path
                    )));
                }
            } else {
                expected_parameter_end = Some(operand_index);
            }
            replace_dynamic_marker(sql, site, &rendered)?;
        }
        if input.sites.is_empty() {
            return Err(ExecuteError::InvalidDynamicMetadata(format!(
                "input `{}` has no SQL usage sites",
                input.path
            )));
        }
    }
    Ok(())
}

fn replace_dynamic_marker(
    sql: &mut String,
    site: &DynamicInputSite,
    rendered: &str,
) -> Result<(), ExecuteError> {
    let mut occurrences = sql.match_indices(&site.marker);
    if occurrences.next().is_none() || occurrences.next().is_some() {
        return Err(ExecuteError::InvalidDynamicMetadata(format!(
            "marker `{}` does not identify exactly one SQL site",
            site.marker
        )));
    }
    *sql = sql.replacen(&site.marker, rendered, 1);
    Ok(())
}

fn render_dynamic_predicate(
    input: &DynamicInputMetadata,
    site: &DynamicInputSite,
    value: &Value,
    path: &str,
    parameters: &mut Vec<BoundParameter>,
    operand_index: &mut usize,
) -> Result<Option<String>, ExecuteError> {
    let Some(object) = value.as_object() else {
        return Err(invalid(path, "a dynamic predicate object"));
    };
    let mut keys = object.keys().map(|key| key.as_str()).collect::<Vec<_>>();
    keys.sort_unstable();
    let mut predicates = Vec::new();
    for key in keys {
        let Some(value) = object.get(key) else {
            continue;
        };
        match key {
            "and" => {
                let Some(items) = value.as_array() else {
                    return Err(invalid(&format!("{path}.and"), "an array of predicates"));
                };
                let mut children = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    if let Some(child) = render_dynamic_predicate(
                        input,
                        site,
                        item,
                        &format!("{path}.and[{index}]"),
                        parameters,
                        operand_index,
                    )? {
                        children.push(child);
                    }
                }
                if !children.is_empty() {
                    predicates.push(parenthesized_join(children, " AND "));
                }
            }
            "or" => {
                let Some(items) = value.as_array() else {
                    return Err(invalid(&format!("{path}.or"), "an array of predicates"));
                };
                let mut children = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    if let Some(child) = render_dynamic_predicate(
                        input,
                        site,
                        item,
                        &format!("{path}.or[{index}]"),
                        parameters,
                        operand_index,
                    )? {
                        children.push(child);
                    }
                }
                predicates.push(if children.is_empty() {
                    "FALSE".to_string()
                } else {
                    parenthesized_join(children, " OR ")
                });
            }
            "not" => {
                if let Some(child) = render_dynamic_predicate(
                    input,
                    site,
                    value,
                    &format!("{path}.not"),
                    parameters,
                    operand_index,
                )? {
                    predicates.push(format!("NOT ({child})"));
                }
            }
            field_key => {
                let field = dynamic_site_field(input, site, field_key, path)?;
                let Some(operators) = value.as_object() else {
                    return Err(invalid(
                        &format!("{path}.{field_key}"),
                        "a dynamic field-operator object",
                    ));
                };
                let mut operator_keys =
                    operators.keys().map(|key| key.as_str()).collect::<Vec<_>>();
                operator_keys.sort_unstable();
                for operator_name in operator_keys {
                    let Some(operand) = operators.get(operator_name) else {
                        continue;
                    };
                    let Some(operator) = field
                        .operators
                        .iter()
                        .find(|operator| operator.name == operator_name)
                    else {
                        return Err(invalid(
                            &format!("{path}.{field_key}.{operator_name}"),
                            "a declared dynamic predicate operator",
                        ));
                    };
                    let operand_path = format!("{path}.{field_key}.{operator_name}");
                    let predicate = match operator.value_kind.as_str() {
                        "scalar" => {
                            let field = dynamic_field(input, field_key)?;
                            validate_dynamic_scalar(field, operand, &operand_path)?;
                            let placeholder = dynamic_parameter(
                                parameters,
                                operand_index,
                                &operand_path,
                                field,
                                false,
                                operand,
                            )?;
                            format!(
                                "{}{placeholder}{}",
                                operator.before_value.as_deref().unwrap_or_default(),
                                operator.after_value.as_deref().unwrap_or_default()
                            )
                        }
                        "collection" => {
                            let field = dynamic_field(input, field_key)?;
                            validate_dynamic_collection(field, operand, &operand_path)?;
                            let Some(items) = operand.as_array() else {
                                return Err(invalid(&operand_path, "a collection"));
                            };
                            if items.is_empty() {
                                if operator_name == "in" {
                                    "FALSE".to_string()
                                } else {
                                    "TRUE".to_string()
                                }
                            } else {
                                let placeholder = dynamic_parameter(
                                    parameters,
                                    operand_index,
                                    &operand_path,
                                    field,
                                    true,
                                    operand,
                                )?;
                                format!(
                                    "{}{placeholder}{}",
                                    operator.before_value.as_deref().unwrap_or_default(),
                                    operator.after_value.as_deref().unwrap_or_default()
                                )
                            }
                        }
                        "boolean" => {
                            let Some(value) = operand.as_bool() else {
                                return Err(invalid(&operand_path, "a boolean"));
                            };
                            let value = if value { "true" } else { "false" };
                            operator
                                .cases
                                .iter()
                                .find(|case| case.value == value)
                                .map(|case| case.text.clone())
                                .ok_or_else(|| {
                                    ExecuteError::InvalidDynamicMetadata(format!(
                                        "operator `{operator_name}` has no `{value}` lowering"
                                    ))
                                })?
                        }
                        kind => {
                            return Err(ExecuteError::InvalidDynamicMetadata(format!(
                                "operator `{operator_name}` has unknown value kind `{kind}`"
                            )));
                        }
                    };
                    predicates.push(predicate);
                }
            }
        }
    }
    Ok((!predicates.is_empty()).then(|| parenthesized_join(predicates, " AND ")))
}

fn render_dynamic_order(
    input: &DynamicInputMetadata,
    site: &DynamicInputSite,
    value: &Value,
    path: &str,
) -> Result<Option<String>, ExecuteError> {
    let Some(items) = value.as_array() else {
        return Err(invalid(path, "an array of dynamic order entries"));
    };
    let mut rendered = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_path = format!("{path}[{index}]");
        let Some(object) = item.as_object() else {
            return Err(invalid(&item_path, "a single-field order object"));
        };
        if object.len() != 1 {
            return Err(invalid(&item_path, "a single-field order object"));
        }
        let Some((field_key, direction)) = object.iter().next() else {
            return Err(invalid(&item_path, "a single-field order object"));
        };
        let field = dynamic_site_field(input, site, field_key.as_str(), path)?;
        let Some(direction) = string_value(direction) else {
            return Err(invalid(&item_path, "a dynamic order direction"));
        };
        let Some(case) = field.directions.iter().find(|case| case.value == direction) else {
            return Err(invalid(&item_path, "a declared dynamic order direction"));
        };
        rendered.push(case.text.clone());
    }
    Ok((!rendered.is_empty()).then(|| rendered.join(", ")))
}

fn dynamic_site_field<'a>(
    input: &DynamicInputMetadata,
    site: &'a DynamicInputSite,
    key: &str,
    path: &str,
) -> Result<&'a DynamicInputSiteField, ExecuteError> {
    if !input.fields.iter().any(|field| field.key == key) {
        return Err(invalid(
            &format!("{path}.{key}"),
            "a selected dynamic field",
        ));
    }
    site.fields
        .iter()
        .find(|field| field.key == key)
        .ok_or_else(|| {
            ExecuteError::InvalidDynamicMetadata(format!(
                "site `{}` has no lowering for field `{key}`",
                site.marker
            ))
        })
}

fn dynamic_field<'a>(
    input: &'a DynamicInputMetadata,
    key: &str,
) -> Result<&'a DynamicInputField, ExecuteError> {
    input
        .fields
        .iter()
        .find(|field| field.key == key)
        .ok_or_else(|| {
            ExecuteError::InvalidDynamicMetadata(format!(
                "input `{}` has no field `{key}`",
                input.path
            ))
        })
}

fn field_wire(field: &InputField) -> WireEncoding {
    field.wire.encoding
}

fn dynamic_field_wire(field: &DynamicInputField) -> WireEncoding {
    field.wire.encoding
}

fn field_provider_type(field: &InputField) -> Option<(String, String)> {
    field_provider_type_from_wire(&field.wire)
}

fn field_provider_type_from_wire(wire: &WireMetadata) -> Option<(String, String)> {
    let provider = wire.provider_type.as_ref()?;
    Some((provider.schema.clone(), provider.name.clone()))
}

fn dynamic_parameter(
    parameters: &mut Vec<BoundParameter>,
    operand_index: &mut usize,
    path: &str,
    field: &DynamicInputField,
    collection: bool,
    value: &Value,
) -> Result<String, ExecuteError> {
    if *operand_index == parameters.len() {
        parameters.push(BoundParameter {
            path: path.to_string(),
            data_type: field.data_type.clone(),
            wire: dynamic_field_wire(field),
            provider_type: field_provider_type_from_wire(&field.wire),
            collection,
            value: value.clone(),
        });
    } else {
        let Some(existing) = parameters.get(*operand_index) else {
            return Err(ExecuteError::InvalidDynamicMetadata(
                "dynamic operand allocation is not contiguous".to_string(),
            ));
        };
        if existing.data_type != field.data_type
            || existing.wire != dynamic_field_wire(field)
            || existing.provider_type != field_provider_type_from_wire(&field.wire)
            || existing.collection != collection
            || existing.value != *value
        {
            return Err(ExecuteError::InvalidDynamicMetadata(format!(
                "reused dynamic site disagrees at operand `{path}`"
            )));
        }
    }
    *operand_index += 1;
    Ok(format!("${}", *operand_index))
}

fn parenthesized_join(items: Vec<String>, separator: &str) -> String {
    format!("({})", items.join(separator))
}

fn validate_dynamic_collection(
    field: &DynamicInputField,
    value: &Value,
    path: &str,
) -> Result<(), ExecuteError> {
    let Some(items) = value.as_array() else {
        return Err(invalid(
            path,
            &format!("an array of {} values", field.data_type),
        ));
    };
    for item in items {
        if item.is_null() {
            return Err(invalid(path, "a collection without null elements"));
        }
        validate_dynamic_scalar(field, item, path)?;
    }
    Ok(())
}

fn validate_dynamic_scalar(
    field: &DynamicInputField,
    value: &Value,
    path: &str,
) -> Result<(), ExecuteError> {
    let data_type = field.data_type.as_str();
    if value.is_null() {
        return Err(invalid(path, &format!("a non-null {data_type} value")));
    }
    let valid = match dynamic_field_wire(field) {
        WireEncoding::Uuid => {
            string_value(value).is_some_and(|value| Uuid::parse_str(value).is_ok())
        }
        WireEncoding::Text | WireEncoding::TextCast => string_value(value).is_some(),
        WireEncoding::Timestamptz => {
            string_value(value).is_some_and(|value| DateTime::parse_from_rfc3339(value).is_ok())
        }
        WireEncoding::Integer => integer_value(value).is_some_and(is_safe_integer),
        WireEncoding::BigInteger => big_integer_value(value).is_some(),
        WireEncoding::Numeric => {
            string_value(value).is_some_and(|value| BigDecimal::from_str(value).is_ok())
        }
        WireEncoding::Float => float_value(value).is_some_and(f64::is_finite),
        WireEncoding::Boolean => value.as_bool().is_some(),
        WireEncoding::Json => true,
        WireEncoding::Unsupported => {
            return Err(ExecuteError::UnsupportedType {
                path: path.to_string(),
                data_type: data_type.to_string(),
            });
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(path, &format!("a valid {data_type} value")))
    }
}

fn materialized_value<'a>(
    values: &'a BTreeMap<&str, Value>,
    path: &str,
) -> Result<&'a Value, ExecuteError> {
    values
        .get(path)
        .ok_or_else(|| ExecuteError::MissingInput(path.to_string()))
}

fn declared<'a>(
    fields: &BTreeMap<&str, &'a InputField>,
    path: &str,
) -> Result<&'a InputField, ExecuteError> {
    fields
        .get(path)
        .copied()
        .ok_or_else(|| ExecuteError::UndeclaredParameter(path.to_string()))
}

fn input_value(
    field: &InputField,
    bindings: &ExecutionBindings,
) -> Result<Option<Value>, ExecuteError> {
    let (root, path, trusted_context) = field
        .path
        .strip_prefix("context.")
        .map_or((&bindings.variables, field.path.as_str(), false), |path| {
            (&bindings.context, path, true)
        });
    let value = match lookup_path(root, path, &field.path)? {
        Some(value) => value.clone(),
        None if trusted_context => return Err(ExecuteError::MissingInput(field.path.clone())),
        None => match &field.default {
            Some(default) => materialize_default(field, default)?,
            None if field.required => {
                return Err(ExecuteError::MissingInput(field.path.clone()));
            }
            None => return Ok(None),
        },
    };
    if value.is_null() && !field.nullable {
        return Err(invalid(
            &field.path,
            &format!("a non-null {}", field.data_type),
        ));
    }
    validate_safe_integer(field, &value)?;
    validate_input_wire(field, &value)?;
    Ok(Some(value))
}

fn validate_safe_integer(field: &InputField, value: &Value) -> Result<(), ExecuteError> {
    if value.is_null() || field_wire(field) != WireEncoding::Integer {
        return Ok(());
    }
    if field.collection == Some(true) {
        let Some(values) = value.as_array() else {
            return Err(invalid(&field.path, "an array of safe integers"));
        };
        if values
            .iter()
            .all(|value| value.is_null() || integer_value(value).is_some_and(is_safe_integer))
        {
            return Ok(());
        }
        return Err(invalid(&field.path, "an array of safe integers"));
    }
    if integer_value(value).is_some_and(is_safe_integer) {
        Ok(())
    } else {
        Err(invalid(&field.path, "a safe integer"))
    }
}

fn validate_input_wire(field: &InputField, value: &Value) -> Result<(), ExecuteError> {
    if value.is_null()
        || field_wire(field) == WireEncoding::Integer
        || matches!(
            field.data_type.as_str(),
            "dynamic_predicate" | "dynamic_order"
        )
    {
        return Ok(());
    }
    if field.collection == Some(true) {
        let Some(values) = value.as_array() else {
            return Err(invalid(
                &field.path,
                &format!("an array of {}", field.data_type),
            ));
        };
        for value in values {
            if !value.is_null() && !input_scalar_is_valid(field, value) {
                return Err(invalid(
                    &field.path,
                    &format!("an array of valid {} values", field.data_type),
                ));
            }
        }
        return Ok(());
    }
    if input_scalar_is_valid(field, value) {
        Ok(())
    } else {
        Err(invalid(
            &field.path,
            &format!("a valid {}", field.data_type),
        ))
    }
}

fn input_scalar_is_valid(field: &InputField, value: &Value) -> bool {
    match field_wire(field) {
        WireEncoding::Uuid => {
            string_value(value).is_some_and(|value| Uuid::parse_str(value).is_ok())
        }
        WireEncoding::Text | WireEncoding::TextCast => string_value(value).is_some(),
        WireEncoding::Timestamptz => {
            string_value(value).is_some_and(|value| DateTime::parse_from_rfc3339(value).is_ok())
        }
        WireEncoding::BigInteger => big_integer_value(value).is_some(),
        WireEncoding::Numeric => {
            string_value(value).is_some_and(|value| BigDecimal::from_str(value).is_ok())
        }
        WireEncoding::Float => float_value(value).is_some(),
        WireEncoding::Boolean => value.as_bool().is_some(),
        WireEncoding::Json => true,
        WireEncoding::Integer | WireEncoding::Unsupported => false,
    }
}

fn lookup_path<'a>(
    root: &'a Value,
    path: &str,
    field_path: &str,
) -> Result<Option<&'a Value>, ExecuteError> {
    let mut current = root;
    for segment in path.split('.') {
        let Some(object) = current.as_object() else {
            return Err(invalid(field_path, "an object input envelope"));
        };
        let Some(value) = object.get(segment) else {
            return Ok(None);
        };
        current = value;
    }
    Ok(Some(current))
}

fn materialize_default(field: &InputField, default: &InputDefault) -> Result<Value, ExecuteError> {
    match default.kind.as_str() {
        "string" if field.collection != Some(true) => {
            let value = default
                .value
                .clone()
                .ok_or_else(|| invalid(&field.path, "a valid string default"))?;
            if !field.enum_values.is_empty() && !field.enum_values.contains(&value) {
                return Err(invalid(&field.path, "a declared string default"));
            }
            Ok(Value::from(value))
        }
        "number" => {
            if field.collection == Some(true) {
                return Err(invalid(&field.path, "a compatible number default"));
            }
            let Some(value) = default.value.as_deref() else {
                return Err(invalid(&field.path, "a valid number default"));
            };
            match field.data_type.as_str() {
                "int" => value
                    .parse::<i64>()
                    .ok()
                    .filter(|value| is_safe_integer(*value))
                    .map(Value::from)
                    .ok_or_else(|| invalid(&field.path, "a safe integer default")),
                "float" => value
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .map(Value::from)
                    .ok_or_else(|| invalid(&field.path, "a finite float default")),
                "numeric" => BigDecimal::from_str(value)
                    .map(|_| Value::from(value))
                    .map_err(|_| invalid(&field.path, "a numeric default")),
                _ => Err(invalid(&field.path, "a compatible number default")),
            }
        }
        "boolean" if field.collection != Some(true) && field.data_type == "boolean" => default
            .boolean
            .map(Value::from)
            .ok_or_else(|| invalid(&field.path, "a valid boolean default")),
        "null" => Ok(Value::NULL),
        "collection" if field.collection == Some(true) || field.data_type == "dynamic_order" => {
            let items = default
                .items
                .as_ref()
                .ok_or_else(|| invalid(&field.path, "a valid collection default"))?;
            if field.data_type == "dynamic_order" && !items.is_empty() {
                return Err(invalid(&field.path, "an empty dynamic order default"));
            }
            let mut item_field = field.clone();
            item_field.collection = None;
            items
                .iter()
                .map(|item| {
                    if matches!(item.kind.as_str(), "null" | "collection" | "empty_object") {
                        return Err(invalid(&field.path, "a valid collection default"));
                    }
                    materialize_default(&item_field, item)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|items| items.into_iter().collect())
        }
        "empty_object"
            if field.collection != Some(true) && field.data_type == "dynamic_predicate" =>
        {
            Ok(VObject::new().into())
        }
        "string" => Err(invalid(&field.path, "a compatible string default")),
        "boolean" => Err(invalid(&field.path, "a compatible boolean default")),
        "collection" => Err(invalid(&field.path, "a compatible collection default")),
        "empty_object" => Err(invalid(&field.path, "a dynamic predicate default")),
        _ => Err(invalid(&field.path, "a recognized input default")),
    }
}

type Query<'q> = QueryScalar<'q, Postgres, Option<String>, PgArguments>;

fn bind<'q>(query: Query<'q>, parameter: &'q BoundParameter) -> Result<Query<'q>, ExecuteError> {
    if parameter.collection {
        bind_collection(query, parameter)
    } else {
        bind_scalar(query, parameter)
    }
}

fn invalid(path: &str, expected: &str) -> ExecuteError {
    ExecuteError::InvalidInput {
        path: path.to_string(),
        expected: expected.to_string(),
    }
}

fn database_error_is_class(error: &sqlx::Error, class: &str) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code.starts_with(class))
}

fn bind_scalar<'q>(
    query: Query<'q>,
    parameter: &'q BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    let value = &parameter.value;
    let error = || invalid(&parameter.path, &parameter.data_type);
    if value.is_null() {
        return bind_null_scalar(query, parameter);
    }
    match parameter.wire {
        WireEncoding::Uuid => Ok(query
            .bind(Uuid::parse_str(string_value(value).ok_or_else(error)?).map_err(|_| error())?)),
        WireEncoding::Text | WireEncoding::TextCast => {
            Ok(query.bind(string_value(value).ok_or_else(error)?))
        }
        WireEncoding::Timestamptz => Ok(query.bind(
            DateTime::parse_from_rfc3339(string_value(value).ok_or_else(error)?)
                .map_err(|_| error())?,
        )),
        WireEncoding::Integer => Ok(query.bind(integer_value(value).ok_or_else(error)?)),
        WireEncoding::BigInteger => Ok(query.bind(big_integer_value(value).ok_or_else(error)?)),
        WireEncoding::Numeric => Ok(query.bind(
            BigDecimal::from_str(string_value(value).ok_or_else(error)?).map_err(|_| error())?,
        )),
        WireEncoding::Float => Ok(query.bind(float_value(value).ok_or_else(error)?)),
        WireEncoding::Boolean => Ok(query.bind(value.as_bool().ok_or_else(error)?)),
        WireEncoding::Json => Ok(query.bind(Json(json_value(parameter)?))),
        WireEncoding::Unsupported => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: parameter.data_type.clone(),
        }),
    }
}

fn bind_null_scalar<'q>(
    query: Query<'q>,
    parameter: &BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    match parameter.wire {
        WireEncoding::Uuid => Ok(query.bind(None::<Uuid>)),
        WireEncoding::Text | WireEncoding::TextCast => Ok(query.bind(None::<String>)),
        WireEncoding::Timestamptz => Ok(query.bind(None::<DateTime<FixedOffset>>)),
        WireEncoding::Integer => Ok(query.bind(None::<i64>)),
        WireEncoding::BigInteger => Ok(query.bind(None::<i64>)),
        WireEncoding::Numeric => Ok(query.bind(None::<BigDecimal>)),
        WireEncoding::Float => Ok(query.bind(None::<f64>)),
        WireEncoding::Boolean => Ok(query.bind(None::<bool>)),
        WireEncoding::Json => Ok(query.bind(None::<Json<Box<JsonRawValue>>>)),
        WireEncoding::Unsupported => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: parameter.data_type.clone(),
        }),
    }
}

fn bind_collection<'q>(
    query: Query<'q>,
    parameter: &'q BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    if parameter.value.is_null() {
        return bind_null_collection(query, parameter);
    }
    let Some(values) = parameter.value.as_array() else {
        return Err(invalid(
            &parameter.path,
            &format!("an array of {}", parameter.data_type),
        ));
    };
    let error = || {
        invalid(
            &parameter.path,
            &format!("an array of {}", parameter.data_type),
        )
    };
    match parameter.wire {
        WireEncoding::Uuid => {
            Ok(query.bind(parse_strings(values, Uuid::parse_str).ok_or_else(error)?))
        }
        WireEncoding::Text | WireEncoding::TextCast => Ok(query.bind(
            values
                .iter()
                .map(|value| {
                    if value.is_null() {
                        Some(None)
                    } else {
                        string_value(value).map(|value| Some(value.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        WireEncoding::Timestamptz => {
            Ok(query.bind(parse_strings(values, DateTime::parse_from_rfc3339).ok_or_else(error)?))
        }
        WireEncoding::Integer => Ok(query.bind(
            values
                .iter()
                .map(|value| {
                    integer_value(value)
                        .map(Some)
                        .or(value.is_null().then_some(None))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        WireEncoding::BigInteger => {
            Ok(query.bind(parse_strings(values, str::parse::<i64>).ok_or_else(error)?))
        }
        WireEncoding::Numeric => {
            Ok(query.bind(parse_strings(values, BigDecimal::from_str).ok_or_else(error)?))
        }
        WireEncoding::Float => Ok(query.bind(
            values
                .iter()
                .map(|value| {
                    float_value(value)
                        .map(Some)
                        .or(value.is_null().then_some(None))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        WireEncoding::Boolean => Ok(query.bind(
            values
                .iter()
                .map(|value| {
                    value
                        .as_bool()
                        .map(Some)
                        .or(value.is_null().then_some(None))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        WireEncoding::Json => {
            let values = values
                .iter()
                .map(|value| {
                    if value.is_null() {
                        Ok(None)
                    } else {
                        json_raw_value(&parameter.path, value).map(|value| Some(Json(value)))
                    }
                })
                .collect::<Result<Vec<_>, ExecuteError>>()?;
            Ok(query.bind(values))
        }
        WireEncoding::Unsupported => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: parameter.data_type.clone(),
        }),
    }
}

fn bind_null_collection<'q>(
    query: Query<'q>,
    parameter: &BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    match parameter.wire {
        WireEncoding::Uuid => Ok(query.bind(None::<Vec<Option<Uuid>>>)),
        WireEncoding::Text | WireEncoding::TextCast => Ok(query.bind(None::<Vec<Option<String>>>)),
        WireEncoding::Timestamptz => Ok(query.bind(None::<Vec<Option<DateTime<FixedOffset>>>>)),
        WireEncoding::Integer => Ok(query.bind(None::<Vec<Option<i64>>>)),
        WireEncoding::BigInteger => Ok(query.bind(None::<Vec<Option<i64>>>)),
        WireEncoding::Numeric => Ok(query.bind(None::<Vec<Option<BigDecimal>>>)),
        WireEncoding::Float => Ok(query.bind(None::<Vec<Option<f64>>>)),
        WireEncoding::Boolean => Ok(query.bind(None::<Vec<Option<bool>>>)),
        WireEncoding::Json => Ok(query.bind(None::<Vec<Option<Json<Box<JsonRawValue>>>>>)),
        WireEncoding::Unsupported => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: parameter.data_type.clone(),
        }),
    }
}

fn parse_strings<T, E>(
    values: &VArray,
    parse: impl Fn(&str) -> Result<T, E>,
) -> Option<Vec<Option<T>>> {
    values
        .iter()
        .map(|value| {
            if value.is_null() {
                Some(None)
            } else {
                string_value(value)
                    .and_then(|value| parse(value).ok())
                    .map(Some)
            }
        })
        .collect()
}

fn string_value(value: &Value) -> Option<&str> {
    value.as_string().map(|value| value.as_str())
}

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_number()
        .filter(|value| value.is_integer())
        .and_then(|value| value.to_i64())
}

fn big_integer_value(value: &Value) -> Option<i64> {
    string_value(value)?.parse().ok()
}

fn is_safe_integer(value: i64) -> bool {
    (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value)
}

fn float_value(value: &Value) -> Option<f64> {
    let value = value.as_number()?.to_f64_lossy();
    value.is_finite().then_some(value)
}

fn json_value(parameter: &BoundParameter) -> Result<Box<JsonRawValue>, ExecuteError> {
    json_raw_value(&parameter.path, &parameter.value)
}

fn json_raw_value(path: &str, value: &Value) -> Result<Box<JsonRawValue>, ExecuteError> {
    let serialized =
        facet_json::to_string(value).map_err(|error| ExecuteError::InvalidJsonParameter {
            path: path.to_string(),
            message: error.to_string(),
        })?;
    JsonRawValue::from_string(serialized).map_err(|error| ExecuteError::InvalidJsonParameter {
        path: path.to_string(),
        message: error.to_string(),
    })
}

fn sort_object_keys(value: &mut Value) {
    if let Some(array) = value.as_array_mut() {
        for value in array.as_mut_slice() {
            sort_object_keys(value);
        }
    } else if let Some(object) = value.as_object_mut() {
        for value in object.values_mut() {
            sort_object_keys(value);
        }
        let mut entries = object
            .iter()
            .map(|(key, value)| (key.as_str().to_string(), value.clone()))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        object.clear();
        for (key, value) in entries {
            object.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use facet_value::Value;

    use super::{float_value, integer_value};

    #[test]
    fn json_number_accessors_preserve_integer_and_float_categories() {
        let integer: Value = facet_json::from_str("5").expect("integer parses");
        let float: Value = facet_json::from_str("5.0").expect("float parses");
        let imprecise_float: Value =
            facet_json::from_str("9007199254740993").expect("large integer parses");

        assert_eq!(integer_value(&integer), Some(5));
        assert_eq!(integer_value(&float), None);
        assert_eq!(float_value(&float), Some(5.0));
        assert_eq!(float_value(&imprecise_float), Some(9_007_199_254_740_992.0));
    }
}
