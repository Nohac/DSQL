use dsql_core::{
    ColumnMetadata, DataType, DatabaseMetadata, ForeignKeyMetadata, ObjectType, SchemaMetadata,
    TableMetadata, TypeMetadata,
};
use sqlx::{FromRow, Pool, Postgres, postgres::PgPoolOptions};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, thiserror::Error)]
pub enum IntrospectionError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, FromRow)]
struct FlatColumn {
    schema_name: String,
    table_name: String,
    column_name: String,
    data_type: String,
    not_null: bool,
    is_primary_key: bool,
    is_unique: bool,
    is_foreign_key: bool,
    fk_schema: Option<String>,
    fk_table: Option<String>,
    fk_column: Option<String>,
    object_type: String,
    is_indexed: bool,
}

#[derive(Debug, FromRow)]
struct PostgresType {
    internal_type: String,
    readable_type: String,
    operation: String,
    operator_schema: String,
}

const PG_INTROSPECTION_QUERY: &str = r#"
SELECT
    ns.nspname AS schema_name,
    tbl.relname AS table_name,
    col.attname AS column_name,
    typ.typname AS data_type,
    col.attnotnull AS not_null,
    CASE WHEN pk.conname IS NOT NULL THEN true ELSE false END AS is_primary_key,
    CASE WHEN uq.conname IS NOT NULL THEN true ELSE false END AS is_unique,
    CASE WHEN fk.conname IS NOT NULL THEN true ELSE false END AS is_foreign_key,
    fk_ns.nspname AS fk_schema,
    fk_tbl.relname AS fk_table,
    fk_col.attname AS fk_column,
    tbl.relkind::text AS object_type,
    EXISTS (
        SELECT 1
        FROM pg_index idx
        JOIN pg_class idx_tbl ON idx.indexrelid = idx_tbl.oid
        WHERE idx.indrelid = tbl.oid
          AND col.attnum = ANY(idx.indkey)
    ) AS is_indexed
FROM
    pg_attribute col
    JOIN pg_class tbl ON col.attrelid = tbl.oid
    JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
    JOIN pg_type typ ON col.atttypid = typ.oid
    LEFT JOIN pg_constraint pk ON col.attnum = ANY(pk.conkey) AND col.attrelid = pk.conrelid AND pk.contype = 'p'
    LEFT JOIN pg_constraint uq ON col.attnum = ANY(uq.conkey) AND col.attrelid = uq.conrelid AND uq.contype = 'u'
    LEFT JOIN pg_constraint fk ON col.attnum = ANY(fk.conkey) AND col.attrelid = fk.conrelid AND fk.contype = 'f'
    LEFT JOIN pg_class fk_tbl ON fk_tbl.oid = fk.confrelid
    LEFT JOIN pg_namespace fk_ns ON fk_ns.oid = fk_tbl.relnamespace
    LEFT JOIN pg_attribute fk_col ON fk_col.attnum = fk.confkey[1] AND fk_col.attrelid = fk_tbl.oid
WHERE
    tbl.relkind IN ('r', 'v', 'm')
    AND ns.nspname NOT LIKE 'pg_%'
    AND ns.nspname <> 'information_schema'
    AND col.attnum > 0
ORDER BY
    ns.nspname, tbl.relname, col.attnum;
"#;

const PG_TYPE_INTROSPECTION_QUERY: &str = r#"
SELECT
    pg_type.typname AS internal_type,
    format_type(pg_type.oid, NULL) AS readable_type,
    pg_operator.oprname AS operation,
    pg_namespace.nspname AS operator_schema
FROM
    pg_type
JOIN
    pg_operator ON pg_operator.oprleft = pg_type.oid
JOIN
    pg_proc ON pg_operator.oprcode = pg_proc.oid
JOIN
    pg_namespace ON pg_proc.pronamespace = pg_namespace.oid
ORDER BY
    pg_type.typname, pg_operator.oprname;
"#;

pub async fn introspect_postgres(
    database_url: &str,
) -> Result<DatabaseMetadata, IntrospectionError> {
    let pool: Pool<Postgres> = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;
    introspect_postgres_pool(&pool).await
}

pub async fn introspect_postgres_pool(
    pool: &Pool<Postgres>,
) -> Result<DatabaseMetadata, IntrospectionError> {
    let schema_rows = sqlx::query_as::<_, FlatColumn>(PG_INTROSPECTION_QUERY)
        .fetch_all(pool)
        .await?;
    let type_rows = sqlx::query_as::<_, PostgresType>(PG_TYPE_INTROSPECTION_QUERY)
        .fetch_all(pool)
        .await?;

    Ok(metadata_from_rows(schema_rows, type_rows))
}

fn metadata_from_rows(
    schema_rows: Vec<FlatColumn>,
    type_rows: Vec<PostgresType>,
) -> DatabaseMetadata {
    let mut type_map = HashMap::<String, TypeMetadata>::new();
    for row in type_rows {
        if let Some(type_metadata) = type_map.get_mut(&row.internal_type) {
            type_metadata.operations.insert(row.operation);
            continue;
        }
        let mut operations = BTreeSet::new();
        operations.insert(row.operation);
        type_map.insert(
            row.internal_type.clone(),
            TypeMetadata {
                internal_type: row.internal_type,
                readable_type: row.readable_type,
                schema: row.operator_schema,
                operations,
            },
        );
    }

    let mut schema_map = HashMap::<String, HashMap<String, TableMetadata>>::new();
    for row in schema_rows {
        let schema = schema_map.entry(row.schema_name.clone()).or_default();
        let table = schema
            .entry(row.table_name.clone())
            .or_insert_with(|| TableMetadata {
                schema: row.schema_name.clone(),
                name: row.table_name.clone(),
                object_type: ObjectType::from_postgres_relkind(&row.object_type),
                columns: Vec::new(),
            });
        let foreign_key = if row.is_foreign_key {
            match (row.fk_schema, row.fk_table, row.fk_column) {
                (Some(schema), Some(table), Some(column)) => Some(ForeignKeyMetadata {
                    schema,
                    table,
                    column,
                }),
                _ => None,
            }
        } else {
            None
        };
        table.columns.push(ColumnMetadata {
            name: row.column_name,
            database_type: row.data_type.clone(),
            data_type: DataType::from_database_type(&row.data_type),
            not_null: row.not_null,
            primary_key: row.is_primary_key,
            unique: row.is_unique,
            indexed: row.is_indexed,
            foreign_key,
        });
    }

    let mut schemas = schema_map
        .into_iter()
        .map(|(name, tables)| {
            let mut tables = tables.into_values().collect::<Vec<_>>();
            tables.sort_by(|left, right| left.name.cmp(&right.name));
            SchemaMetadata { name, tables }
        })
        .collect::<Vec<_>>();
    schemas.sort_by(|left, right| left.name.cmp(&right.name));

    let mut types = type_map.into_values().collect::<Vec<_>>();
    types.sort_by(|left, right| left.internal_type.cmp(&right.internal_type));

    let mut metadata = DatabaseMetadata { schemas, types };
    metadata.canonicalize();
    metadata
}
