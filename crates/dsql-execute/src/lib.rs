//! Native PostgreSQL execution for compiled [`dsql_metadata::OperationMetadata`].

use std::collections::BTreeMap;
use std::str::FromStr;

use dsql_metadata::{InputDefault, InputField, OperationMetadata};
use serde_json::Value;
use sqlx::postgres::{PgArguments, PgPool, PgPoolOptions};
use sqlx::query::QueryScalar;
use sqlx::types::{
    BigDecimal, Json, Uuid,
    chrono::{DateTime, FixedOffset},
};
use sqlx::{AssertSqlSafe, Postgres};

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
            variables: Value::Object(serde_json::Map::new()),
            context: Value::Object(serde_json::Map::new()),
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
    #[error("operation kind `{0}` cannot be executed as a PostgreSQL query")]
    UnsupportedOperationKind(String),
    #[error("operation SQL dialect `{0}` is not PostgreSQL")]
    UnsupportedDialect(String),
    #[error("operation returned invalid JSON: {0}")]
    InvalidOutput(serde_json::Error),
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
            "select ROW_TO_JSON(\"dsql_result\")::text from ({inner}) as \"dsql_result\""
        )));
        for parameter in &materialized.parameters {
            query = bind(query, parameter)?;
        }
        query
            .fetch_one(&self.pool)
            .await?
            .map_or(Ok(Value::Null), |output| {
                serde_json::from_str(&output).map_err(ExecuteError::InvalidOutput)
            })
    }
}

pub fn materialize(
    operation: &OperationMetadata,
    bindings: &ExecutionBindings,
) -> Result<MaterializedOperation, ExecuteError> {
    if operation.kind != "query" {
        return Err(ExecuteError::UnsupportedOperationKind(
            operation.kind.clone(),
        ));
    }
    if operation.sql.dialect != "postgres" {
        return Err(ExecuteError::UnsupportedDialect(
            operation.sql.dialect.clone(),
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
        let Some(value) = value.as_str() else {
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

    let parameters = operation
        .sql
        .parameters
        .iter()
        .map(|parameter| {
            let field = declared(&fields, &parameter.path)?;
            Ok(BoundParameter {
                path: parameter.path.clone(),
                data_type: field.data_type.clone(),
                collection: field.collection == Some(true),
                value: materialized_value(&values, &parameter.path)?.clone(),
            })
        })
        .collect::<Result<Vec<_>, ExecuteError>>()?;

    Ok(MaterializedOperation { sql, parameters })
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
    Ok(Some(value))
}

fn lookup_path<'a>(
    root: &'a Value,
    path: &str,
    field_path: &str,
) -> Result<Option<&'a Value>, ExecuteError> {
    let mut current = root;
    for segment in path.split('.') {
        let Value::Object(object) = current else {
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
            Ok(Value::String(value))
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
                    .map(Value::from)
                    .ok_or_else(|| invalid(&field.path, "an integer default")),
                "float" => value
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .ok_or_else(|| invalid(&field.path, "a finite float default")),
                "numeric" => BigDecimal::from_str(value)
                    .map(|_| Value::String(value.to_string()))
                    .map_err(|_| invalid(&field.path, "a numeric default")),
                _ => Err(invalid(&field.path, "a compatible number default")),
            }
        }
        "boolean" if field.collection != Some(true) && field.data_type == "boolean" => default
            .boolean
            .map(Value::Bool)
            .ok_or_else(|| invalid(&field.path, "a valid boolean default")),
        "null" => Ok(Value::Null),
        "collection" if field.collection == Some(true) => {
            let items = default
                .items
                .as_ref()
                .ok_or_else(|| invalid(&field.path, "a valid collection default"))?;
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
                .map(Value::Array)
        }
        "empty_object" if field.collection != Some(true) => {
            Ok(Value::Object(serde_json::Map::new()))
        }
        "string" => Err(invalid(&field.path, "a compatible string default")),
        "boolean" => Err(invalid(&field.path, "a compatible boolean default")),
        "collection" => Err(invalid(&field.path, "a compatible collection default")),
        "empty_object" => Err(invalid(&field.path, "a compatible object default")),
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

fn bind_scalar<'q>(
    query: Query<'q>,
    parameter: &'q BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    let value = &parameter.value;
    let error = || invalid(&parameter.path, &parameter.data_type);
    if value.is_null() {
        return bind_null_scalar(query, parameter);
    }
    match parameter.data_type.as_str() {
        "uuid" => Ok(
            query.bind(Uuid::parse_str(value.as_str().ok_or_else(error)?).map_err(|_| error())?)
        ),
        "text" => Ok(query.bind(value.as_str().ok_or_else(error)?)),
        "timestamptz" => Ok(query.bind(
            DateTime::parse_from_rfc3339(value.as_str().ok_or_else(error)?).map_err(|_| error())?,
        )),
        "int" => Ok(query.bind(value.as_i64().ok_or_else(error)?)),
        "numeric" => Ok(query
            .bind(BigDecimal::from_str(value.as_str().ok_or_else(error)?).map_err(|_| error())?)),
        "float" => Ok(query.bind(value.as_f64().ok_or_else(error)?)),
        "boolean" => Ok(query.bind(value.as_bool().ok_or_else(error)?)),
        "json" => Ok(query.bind(Json(value.clone()))),
        data_type => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: data_type.to_string(),
        }),
    }
}

fn bind_null_scalar<'q>(
    query: Query<'q>,
    parameter: &BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    match parameter.data_type.as_str() {
        "uuid" => Ok(query.bind(None::<Uuid>)),
        "text" => Ok(query.bind(None::<String>)),
        "timestamptz" => Ok(query.bind(None::<DateTime<FixedOffset>>)),
        "int" => Ok(query.bind(None::<i64>)),
        "numeric" => Ok(query.bind(None::<BigDecimal>)),
        "float" => Ok(query.bind(None::<f64>)),
        "boolean" => Ok(query.bind(None::<bool>)),
        "json" => Ok(query.bind(None::<Json<Value>>)),
        data_type => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: data_type.to_string(),
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
    match parameter.data_type.as_str() {
        "uuid" => Ok(query.bind(parse_strings(values, Uuid::parse_str).ok_or_else(error)?)),
        "text" => Ok(query.bind(
            values
                .iter()
                .map(|value| {
                    if value.is_null() {
                        Some(None)
                    } else {
                        value.as_str().map(|value| Some(value.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "timestamptz" => {
            Ok(query.bind(parse_strings(values, DateTime::parse_from_rfc3339).ok_or_else(error)?))
        }
        "int" => Ok(query.bind(
            values
                .iter()
                .map(|value| value.as_i64().map(Some).or(value.is_null().then_some(None)))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "numeric" => Ok(query.bind(parse_strings(values, BigDecimal::from_str).ok_or_else(error)?)),
        "float" => Ok(query.bind(
            values
                .iter()
                .map(|value| value.as_f64().map(Some).or(value.is_null().then_some(None)))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "boolean" => Ok(query.bind(
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
        "json" => Ok(query.bind(
            values
                .iter()
                .map(|value| (!value.is_null()).then(|| Json(value.clone())))
                .collect::<Vec<_>>(),
        )),
        data_type => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: data_type.to_string(),
        }),
    }
}

fn bind_null_collection<'q>(
    query: Query<'q>,
    parameter: &BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
    match parameter.data_type.as_str() {
        "uuid" => Ok(query.bind(None::<Vec<Option<Uuid>>>)),
        "text" => Ok(query.bind(None::<Vec<Option<String>>>)),
        "timestamptz" => Ok(query.bind(None::<Vec<Option<DateTime<FixedOffset>>>>)),
        "int" => Ok(query.bind(None::<Vec<Option<i64>>>)),
        "numeric" => Ok(query.bind(None::<Vec<Option<BigDecimal>>>)),
        "float" => Ok(query.bind(None::<Vec<Option<f64>>>)),
        "boolean" => Ok(query.bind(None::<Vec<Option<bool>>>)),
        "json" => Ok(query.bind(None::<Vec<Option<Json<Value>>>>)),
        data_type => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: data_type.to_string(),
        }),
    }
}

fn parse_strings<T, E>(
    values: &[Value],
    parse: impl Fn(&str) -> Result<T, E>,
) -> Option<Vec<Option<T>>> {
    values
        .iter()
        .map(|value| {
            if value.is_null() {
                Some(None)
            } else {
                value.as_str().and_then(|value| parse(value).ok()).map(Some)
            }
        })
        .collect()
}
