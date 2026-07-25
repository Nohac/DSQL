//! SQL generation from query plans.

mod generate;
mod postgres;

pub use generate::{GeneratedSqlFact, SqlOptions, register_sql};
pub use postgres::{
    GeneratedDynamicInputSite, GeneratedDynamicInputSiteField, GeneratedDynamicPredicateOperator,
    GeneratedDynamicValueKind, GeneratedSql, GeneratedSqlParameter, GeneratedSqlVariant,
    PostgresSqlOptions, SqlGenerationError, generate_postgres_sql,
    generate_postgres_sql_with_options,
};
