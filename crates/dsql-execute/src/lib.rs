//! Native PostgreSQL execution for compiled [`dsql_metadata::OperationMetadata`].

use std::collections::BTreeMap;
use std::str::FromStr;

use dsql_metadata::{InputField, OperationMetadata};
use serde_json::Value;
use sqlx::postgres::{PgArguments, PgPool, PgPoolOptions};
use sqlx::query::QueryScalar;
use sqlx::types::{BigDecimal, Json, Uuid, chrono::DateTime};
use sqlx::{AssertSqlSafe, Postgres};

/// Public inputs and trusted server context for one execution.
#[derive(Clone, Debug, Default)]
pub struct ExecutionBindings {
    /// The metadata-shaped `params` and `input` trees.
    pub variables: Value,
    /// Context fields without the `context` wrapper.
    pub context: Value,
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
    let fields = operation
        .params
        .iter()
        .chain(&operation.input)
        .chain(&operation.context)
        .map(|field| (field.path.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut sql = operation.sql.text.clone();

    for variant in &operation.sql.variants {
        let field = declared(&fields, &variant.path)?;
        let value = input_value(field, bindings)?;
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
                value: input_value(field, bindings)?.clone(),
            })
        })
        .collect::<Result<Vec<_>, ExecuteError>>()?;

    Ok(MaterializedOperation { sql, parameters })
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

fn input_value<'a>(
    field: &InputField,
    bindings: &'a ExecutionBindings,
) -> Result<&'a Value, ExecuteError> {
    let (root, path) = field
        .path
        .strip_prefix("context.")
        .map_or((&bindings.variables, field.path.as_str()), |path| {
            (&bindings.context, path)
        });
    let value = path
        .split('.')
        .try_fold(root, |value, segment| value.get(segment))
        .ok_or_else(|| ExecuteError::MissingInput(field.path.clone()))?;
    if value.is_null() && !field.nullable {
        return Err(invalid(
            &field.path,
            &format!("a non-null {}", field.data_type),
        ));
    }
    Ok(value)
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

fn bind_collection<'q>(
    query: Query<'q>,
    parameter: &'q BoundParameter,
) -> Result<Query<'q>, ExecuteError> {
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
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "timestamptz" => {
            Ok(query.bind(parse_strings(values, DateTime::parse_from_rfc3339).ok_or_else(error)?))
        }
        "int" => Ok(query.bind(
            values
                .iter()
                .map(Value::as_i64)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "numeric" => Ok(query.bind(parse_strings(values, BigDecimal::from_str).ok_or_else(error)?)),
        "float" => Ok(query.bind(
            values
                .iter()
                .map(Value::as_f64)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "boolean" => Ok(query.bind(
            values
                .iter()
                .map(Value::as_bool)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(error)?,
        )),
        "json" => Ok(query.bind(values.iter().cloned().map(Json).collect::<Vec<_>>())),
        data_type => Err(ExecuteError::UnsupportedType {
            path: parameter.path.clone(),
            data_type: data_type.to_string(),
        }),
    }
}

fn parse_strings<T, E>(values: &[Value], parse: impl Fn(&str) -> Result<T, E>) -> Option<Vec<T>> {
    values
        .iter()
        .map(|value| value.as_str().and_then(|value| parse(value).ok()))
        .collect()
}
