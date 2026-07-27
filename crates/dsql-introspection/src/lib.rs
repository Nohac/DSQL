//! PostgreSQL catalog introspection: `pg_catalog` queries flattened into
//! the same [`DatabaseMetadata`] model the schema YAML files carry.

use dsql_core::catalog::{
    ColumnMetadata, DataType, DatabaseMetadata, ForeignKeyConstraintMetadata,
    ForeignKeyReferenceMetadata, IndexKeyCapability, IndexKeyMetadata, IndexMetadata,
    IndexNullsPosition, IndexOrder, IndexOrderDirection, ObjectType, ProviderTypeFacts,
    SchemaMetadata, TableConstraintKind, TableConstraintMetadata, TableMetadata, TypeKey,
    TypeMetadata,
};
use sqlx::{AssertSqlSafe, Executor, FromRow, Pool, Postgres, postgres::PgPoolOptions};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, thiserror::Error)]
pub enum IntrospectionError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(
        "column `{schema}.{table}.{column}` references PostgreSQL type OID {type_oid}, \
         but that type was absent from the same catalog snapshot"
    )]
    IncompleteTypeSnapshot {
        schema: String,
        table: String,
        column: String,
        type_oid: i64,
    },
}

#[derive(Debug, FromRow)]
struct FlatColumn {
    schema_name: String,
    table_name: String,
    table_description: Option<String>,
    column_name: String,
    column_description: Option<String>,
    type_oid: i64,
    type_modifier: i32,
    formatted_type: String,
    not_null: bool,
    object_type: String,
}

#[derive(Debug, FromRow)]
struct PostgresConstraint {
    schema_name: String,
    table_name: String,
    constraint_name: String,
    constraint_kind: String,
    columns: Vec<String>,
}

#[derive(Debug, FromRow)]
struct PostgresForeignKey {
    schema_name: String,
    table_name: String,
    foreign_key_name: String,
    columns: Vec<String>,
    referenced_schema_name: String,
    referenced_table_name: String,
    referenced_columns: Vec<String>,
}

#[derive(Debug, FromRow)]
struct PostgresIndexKey {
    schema_name: String,
    table_name: String,
    index_name: String,
    access_method: String,
    is_unique: bool,
    key_position: i64,
    key_count: i64,
    column_name: String,
    operator_class: Option<String>,
    native_operators: Vec<String>,
    orderable: Option<bool>,
    ascending: Option<bool>,
    nulls_first: Option<bool>,
    included_columns: Vec<String>,
}

#[derive(Debug, FromRow)]
struct PostgresType {
    type_oid: i64,
    internal_type: String,
    readable_type: String,
    type_schema: String,
    type_kind: String,
    type_category: String,
    operations: Vec<String>,
    orderable: bool,
}

const PG_INTROSPECTED_COLUMN_PREDICATE: &str = r#"
    tbl.relkind IN ('r', 'v', 'm')
    AND ns.nspname NOT LIKE 'pg_%'
    AND ns.nspname <> 'information_schema'
    AND col.attnum > 0
"#;

fn pg_introspection_query() -> String {
    format!(
        r#"
SELECT
    ns.nspname AS schema_name,
    tbl.relname AS table_name,
    obj_description(tbl.oid, 'pg_class') AS table_description,
    col.attname AS column_name,
    col_description(tbl.oid, col.attnum) AS column_description,
    col.atttypid::bigint AS type_oid,
    col.atttypmod AS type_modifier,
    pg_catalog.format_type(col.atttypid, col.atttypmod) AS formatted_type,
    col.attnotnull AS not_null,
    tbl.relkind::text AS object_type
FROM
    pg_attribute col
    JOIN pg_class tbl ON col.attrelid = tbl.oid
    JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
    JOIN pg_type typ ON col.atttypid = typ.oid
WHERE
    {PG_INTROSPECTED_COLUMN_PREDICATE}
ORDER BY
    ns.nspname, tbl.relname, col.attnum;
"#
    )
}

const PG_CONSTRAINT_QUERY: &str = r#"
SELECT
    ns.nspname AS schema_name,
    tbl.relname AS table_name,
    con.conname AS constraint_name,
    CASE con.contype
        WHEN 'p' THEN 'primary_key'
        WHEN 'u' THEN 'unique'
    END AS constraint_kind,
    array_agg(col.attname ORDER BY conkey.ordinality) AS columns
FROM
    pg_constraint con
    JOIN pg_class tbl ON con.conrelid = tbl.oid
    JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
    JOIN unnest(con.conkey) WITH ORDINALITY AS conkey(attnum, ordinality) ON true
    JOIN pg_attribute col ON col.attrelid = tbl.oid AND col.attnum = conkey.attnum
WHERE
    con.contype IN ('p', 'u')
    AND ns.nspname NOT LIKE 'pg_%'
    AND ns.nspname <> 'information_schema'
GROUP BY
    ns.nspname, tbl.relname, con.conname, con.contype
ORDER BY
    ns.nspname, tbl.relname, con.conname;
"#;

const PG_FOREIGN_KEY_QUERY: &str = r#"
SELECT
    ns.nspname AS schema_name,
    tbl.relname AS table_name,
    con.conname AS foreign_key_name,
    array_agg(local_col.attname ORDER BY local_key.ordinality) AS columns,
    ref_ns.nspname AS referenced_schema_name,
    ref_tbl.relname AS referenced_table_name,
    array_agg(ref_col.attname ORDER BY local_key.ordinality) AS referenced_columns
FROM
    pg_constraint con
    JOIN pg_class tbl ON con.conrelid = tbl.oid
    JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
    JOIN pg_class ref_tbl ON con.confrelid = ref_tbl.oid
    JOIN pg_namespace ref_ns ON ref_ns.oid = ref_tbl.relnamespace
    JOIN unnest(con.conkey) WITH ORDINALITY AS local_key(attnum, ordinality) ON true
    JOIN unnest(con.confkey) WITH ORDINALITY AS ref_key(attnum, ordinality)
      ON ref_key.ordinality = local_key.ordinality
    JOIN pg_attribute local_col
      ON local_col.attrelid = tbl.oid AND local_col.attnum = local_key.attnum
    JOIN pg_attribute ref_col
      ON ref_col.attrelid = ref_tbl.oid AND ref_col.attnum = ref_key.attnum
WHERE
    con.contype = 'f'
    AND ns.nspname NOT LIKE 'pg_%'
    AND ns.nspname <> 'information_schema'
GROUP BY
    ns.nspname, tbl.relname, con.conname, ref_ns.nspname, ref_tbl.relname
ORDER BY
    ns.nspname, tbl.relname, con.conname;
"#;

const PG_INDEX_QUERY: &str = r#"
SELECT
    ns.nspname AS schema_name,
    tbl.relname AS table_name,
    idx_tbl.relname AS index_name,
    am.amname AS access_method,
    idx.indisunique AS is_unique,
    idx_key.ordinality::bigint AS key_position,
    idx.indnkeyatts::bigint AS key_count,
    col.attname AS column_name,
    opc_ns.nspname || '.' || opc.opcname AS operator_class,
    COALESCE(
      array_agg(DISTINCT opr.oprname::text)
        FILTER (WHERE opr.oprname IS NOT NULL),
      ARRAY[]::text[]
    ) AS native_operators,
    pg_index_column_has_property(
      idx.indexrelid, idx_key.ordinality::int, 'orderable'
    ) AS orderable,
    pg_index_column_has_property(
      idx.indexrelid, idx_key.ordinality::int, 'asc'
    ) AS ascending,
    pg_index_column_has_property(
      idx.indexrelid, idx_key.ordinality::int, 'nulls_first'
    ) AS nulls_first,
    ARRAY(
      SELECT include_col.attname::text
      FROM unnest(idx.indkey::int2[]) WITH ORDINALITY
        AS include_key(attnum, ordinality)
      JOIN pg_attribute include_col
        ON include_col.attrelid = tbl.oid
        AND include_col.attnum = include_key.attnum
      WHERE include_key.ordinality > idx.indnkeyatts
      ORDER BY include_key.ordinality
    ) AS included_columns
FROM
    pg_index idx
    JOIN pg_class tbl ON idx.indrelid = tbl.oid
    JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
    JOIN pg_class idx_tbl ON idx.indexrelid = idx_tbl.oid
    JOIN pg_am am ON am.oid = idx_tbl.relam
    JOIN unnest(idx.indkey::int2[]) WITH ORDINALITY AS idx_key(attnum, ordinality)
      ON idx_key.attnum > 0
    JOIN pg_attribute col ON col.attrelid = tbl.oid AND col.attnum = idx_key.attnum
    LEFT JOIN unnest(idx.indclass::oid[]) WITH ORDINALITY AS idx_class(opclass, ordinality)
      ON idx_class.ordinality = idx_key.ordinality
    LEFT JOIN pg_opclass opc ON opc.oid = idx_class.opclass
    LEFT JOIN pg_namespace opc_ns ON opc_ns.oid = opc.opcnamespace
    LEFT JOIN pg_amop amop ON amop.amopfamily = opc.opcfamily
    LEFT JOIN pg_operator opr ON opr.oid = amop.amopopr
WHERE
    tbl.relkind IN ('r', 'v', 'm')
    AND ns.nspname NOT LIKE 'pg_%'
    AND ns.nspname <> 'information_schema'
    AND idx.indisvalid
    AND idx.indisready
    AND idx.indexprs IS NULL
    AND idx.indpred IS NULL
    AND idx_key.ordinality <= idx.indnkeyatts
GROUP BY
    ns.nspname, tbl.relname, idx_tbl.relname, am.amname, idx.indisunique,
    idx_key.ordinality, col.attname, opc_ns.nspname, opc.opcname,
    idx.indexrelid, idx.indkey, idx.indnkeyatts, tbl.oid
ORDER BY
    ns.nspname, tbl.relname, idx_tbl.relname, idx_key.ordinality;
"#;

fn pg_type_introspection_query() -> String {
    format!(
        r#"
WITH RECURSIVE domain_chain(source_oid, candidate_oid) AS (
    SELECT typ.oid, typ.oid
    FROM pg_type typ

    UNION

    SELECT chain.source_oid, base.oid
    FROM domain_chain chain
    JOIN pg_type candidate ON candidate.oid = chain.candidate_oid
    JOIN pg_type base ON base.oid = candidate.typbasetype
    WHERE candidate.typtype = 'd'
),
effective_type AS (
    -- A PostgreSQL domain chain has exactly one non-domain terminal.
    SELECT DISTINCT ON (chain.source_oid)
        chain.source_oid,
        candidate.oid,
        candidate.typtype,
        candidate.typcategory
    FROM domain_chain chain
    JOIN pg_type candidate ON candidate.oid = chain.candidate_oid
    WHERE candidate.typtype <> 'd'
    ORDER BY chain.source_oid
),
operand_candidates(source_oid, candidate_oid) AS (
    SELECT source_oid, candidate_oid
    FROM domain_chain

    UNION

    SELECT chain.source_oid, cast_row.casttarget
    FROM domain_chain chain
    JOIN pg_cast cast_row ON cast_row.castsource = chain.candidate_oid
    WHERE cast_row.castcontext = 'i'

    UNION

    SELECT effective.source_oid, polymorphic.oid
    FROM effective_type effective
    JOIN pg_type polymorphic ON polymorphic.typname = CASE
        WHEN effective.typtype = 'e' THEN 'anyenum'
        WHEN effective.typtype = 'r' THEN 'anyrange'
        WHEN effective.typtype = 'm' THEN 'anymultirange'
        WHEN effective.typcategory = 'A' THEN 'anyarray'
    END
    JOIN pg_namespace polymorphic_ns ON polymorphic_ns.oid = polymorphic.typnamespace
    WHERE polymorphic_ns.nspname = 'pg_catalog'
),
native_operations(source_oid, operation) AS (
    SELECT candidates.source_oid, operator.oprname::text
    FROM operand_candidates candidates
    JOIN pg_operator operator
      ON operator.oprleft = candidates.candidate_oid
      AND operator.oprright = candidates.candidate_oid

    UNION

    SELECT candidates.source_oid, operator.oprname::text
    FROM operand_candidates candidates
    JOIN pg_opclass operator_class
      ON operator_class.opcintype = candidates.candidate_oid
      AND operator_class.opcdefault
    JOIN pg_amop family_operator
      ON family_operator.amopfamily = operator_class.opcfamily
      AND family_operator.amoplefttype = candidates.candidate_oid
      AND family_operator.amoprighttype = candidates.candidate_oid
    JOIN pg_operator operator ON operator.oid = family_operator.amopopr
)
SELECT
    typ.oid::bigint AS type_oid,
    typ.typname AS internal_type,
    pg_catalog.format_type(typ.oid, NULL) AS readable_type,
    type_ns.nspname AS type_schema,
    typ.typtype::text AS type_kind,
    typ.typcategory::text AS type_category,
    COALESCE(
        array_agg(DISTINCT native.operation ORDER BY native.operation)
            FILTER (WHERE native.operation IS NOT NULL),
        ARRAY[]::text[]
    ) AS operations,
    EXISTS (
        SELECT 1
        FROM operand_candidates candidates
        JOIN pg_opclass operator_class
          ON operator_class.opcintype = candidates.candidate_oid
          AND operator_class.opcdefault
        JOIN pg_am access_method ON access_method.oid = operator_class.opcmethod
        WHERE candidates.source_oid = typ.oid
          AND access_method.amname = 'btree'
    ) AS orderable
FROM
    pg_type typ
    JOIN pg_namespace type_ns ON type_ns.oid = typ.typnamespace
    LEFT JOIN native_operations native ON native.source_oid = typ.oid
    LEFT JOIN pg_class type_rel ON type_rel.oid = typ.typrelid
    LEFT JOIN pg_type element_type ON element_type.oid = typ.typelem
    LEFT JOIN pg_class element_rel ON element_rel.oid = element_type.typrelid
WHERE
    type_ns.nspname <> 'pg_toast'
    AND (
        (
            (typ.typtype <> 'c' OR type_rel.relkind = 'c')
            AND (
                typ.typelem = 0
                OR element_type.typtype <> 'c'
                OR element_rel.relkind = 'c'
            )
        )
        OR EXISTS (
            SELECT 1
            FROM pg_attribute col
            JOIN pg_class tbl ON col.attrelid = tbl.oid
            JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
            WHERE col.atttypid = typ.oid
                AND {PG_INTROSPECTED_COLUMN_PREDICATE}
        )
    )
GROUP BY
    typ.oid, typ.typname, typ.typtype, typ.typcategory, type_ns.nspname
ORDER BY
    type_ns.nspname, typ.typname;
"#
    )
}

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
    let mut transaction = pool.begin().await?;
    transaction
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .await?;
    transaction
        .execute("SET LOCAL search_path = pg_catalog")
        .await?;
    let column_query = pg_introspection_query();
    let schema_rows = sqlx::query_as::<_, FlatColumn>(AssertSqlSafe(column_query))
        .fetch_all(&mut *transaction)
        .await?;
    let constraint_rows = sqlx::query_as::<_, PostgresConstraint>(PG_CONSTRAINT_QUERY)
        .fetch_all(&mut *transaction)
        .await?;
    let foreign_key_rows = sqlx::query_as::<_, PostgresForeignKey>(PG_FOREIGN_KEY_QUERY)
        .fetch_all(&mut *transaction)
        .await?;
    let index_rows = sqlx::query_as::<_, PostgresIndexKey>(PG_INDEX_QUERY)
        .fetch_all(&mut *transaction)
        .await?;
    let type_query = pg_type_introspection_query();
    let type_rows = sqlx::query_as::<_, PostgresType>(AssertSqlSafe(type_query))
        .fetch_all(&mut *transaction)
        .await?;

    let metadata = metadata_from_rows(
        schema_rows,
        constraint_rows,
        foreign_key_rows,
        index_rows,
        type_rows,
    )?;
    transaction.commit().await?;
    Ok(metadata)
}

fn metadata_from_rows(
    schema_rows: Vec<FlatColumn>,
    constraint_rows: Vec<PostgresConstraint>,
    foreign_key_rows: Vec<PostgresForeignKey>,
    index_rows: Vec<PostgresIndexKey>,
    type_rows: Vec<PostgresType>,
) -> Result<DatabaseMetadata, IntrospectionError> {
    let mut type_map = BTreeMap::<TypeKey, TypeMetadata>::new();
    let mut type_keys_by_oid = HashMap::<i64, TypeKey>::new();
    for row in type_rows {
        let key = TypeKey {
            schema: row.type_schema,
            name: row.internal_type,
        };
        type_keys_by_oid.insert(row.type_oid, key.clone());
        type_map
            .entry(key.clone())
            .or_insert_with(|| TypeMetadata {
                internal_type: key.name,
                readable_type: row.readable_type,
                schema: key.schema,
                provider: Some(ProviderTypeFacts {
                    kind: row.type_kind,
                    category: row.type_category,
                    orderable: row.orderable,
                }),
                operations: BTreeSet::new(),
            })
            .operations
            .extend(row.operations);
    }

    let mut schema_map = HashMap::<String, HashMap<String, TableMetadata>>::new();
    for row in schema_rows {
        let provider_type = type_keys_by_oid
            .get(&row.type_oid)
            .cloned()
            .ok_or_else(|| IntrospectionError::IncompleteTypeSnapshot {
                schema: row.schema_name.clone(),
                table: row.table_name.clone(),
                column: row.column_name.clone(),
                type_oid: row.type_oid,
            })?;
        let schema = schema_map.entry(row.schema_name.clone()).or_default();
        let table = schema
            .entry(row.table_name.clone())
            .or_insert_with(|| TableMetadata {
                schema: row.schema_name.clone(),
                name: row.table_name.clone(),
                object_type: ObjectType::from_postgres_relkind(&row.object_type),
                description: row.table_description,
                columns: Vec::new(),
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            });
        table.columns.push(ColumnMetadata {
            name: row.column_name,
            description: row.column_description,
            database_type: provider_type.name.clone(),
            data_type: DataType::from_database_type(&provider_type.name),
            provider_type,
            formatted_type: Some(row.formatted_type),
            type_modifier: Some(row.type_modifier),
            not_null: row.not_null,
        });
    }

    for row in constraint_rows {
        let Some(table) = table_mut(&mut schema_map, &row.schema_name, &row.table_name) else {
            continue;
        };
        let kind = match row.constraint_kind.as_str() {
            "primary_key" => TableConstraintKind::PrimaryKey,
            "unique" => TableConstraintKind::Unique,
            _ => continue,
        };
        table.constraints.push(TableConstraintMetadata {
            name: Some(row.constraint_name),
            kind,
            columns: row.columns,
        });
    }

    for row in foreign_key_rows {
        let Some(table) = table_mut(&mut schema_map, &row.schema_name, &row.table_name) else {
            continue;
        };
        table.foreign_keys.push(ForeignKeyConstraintMetadata {
            name: Some(row.foreign_key_name),
            columns: row.columns,
            references: ForeignKeyReferenceMetadata {
                schema: row.referenced_schema_name,
                table: row.referenced_table_name,
                columns: row.referenced_columns,
            },
        });
    }

    struct IndexAccumulator {
        metadata: IndexMetadata,
        expected_key_count: Option<usize>,
        valid: bool,
    }

    let mut indexes = BTreeMap::<(String, String, String), IndexAccumulator>::new();
    for row in index_rows {
        let mut capabilities = Vec::new();
        if row.native_operators.iter().any(|operator| operator == "=") {
            capabilities.push(IndexKeyCapability::Equality);
        }
        if row
            .native_operators
            .iter()
            .any(|operator| matches!(operator.as_str(), "<" | "<=" | ">" | ">="))
        {
            capabilities.push(IndexKeyCapability::Range);
        }
        if row.native_operators.iter().any(|operator| operator == "~~") {
            capabilities.push(IndexKeyCapability::Like);
        }
        let order = (row.orderable == Some(true)).then(|| IndexOrder {
            direction: if row.ascending == Some(false) {
                IndexOrderDirection::Desc
            } else {
                IndexOrderDirection::Asc
            },
            nulls: if row.nulls_first == Some(true) {
                IndexNullsPosition::First
            } else {
                IndexNullsPosition::Last
            },
        });
        let key = (
            row.schema_name.clone(),
            row.table_name.clone(),
            row.index_name.clone(),
        );
        let expected_key_count = usize::try_from(row.key_count)
            .ok()
            .filter(|count| *count > 0);
        let index = indexes.entry(key).or_insert_with(|| IndexAccumulator {
            metadata: IndexMetadata {
                name: Some(row.index_name),
                access_method: row.access_method,
                keys: Vec::new(),
                included_columns: row.included_columns,
                unique: row.is_unique,
            },
            expected_key_count,
            valid: expected_key_count.is_some(),
        });
        if index.expected_key_count != expected_key_count {
            index.valid = false;
        }
        let position = usize::try_from(row.key_position.saturating_sub(1)).unwrap_or(usize::MAX);
        if index.valid && position == index.metadata.keys.len() {
            index.metadata.keys.push(IndexKeyMetadata {
                column: row.column_name,
                operator_class: row.operator_class,
                capabilities,
                order,
            });
        } else {
            index.valid = false;
        }
    }
    for ((schema_name, table_name, _), index) in indexes {
        if !index.valid || index.expected_key_count != Some(index.metadata.keys.len()) {
            continue;
        }
        let Some(table) = table_mut(&mut schema_map, &schema_name, &table_name) else {
            continue;
        };
        table.indexes.push(index.metadata);
    }

    let mut metadata = DatabaseMetadata {
        schemas: schema_map
            .into_iter()
            .map(|(name, tables)| SchemaMetadata {
                name,
                tables: tables.into_values().collect(),
            })
            .collect(),
        types: type_map.into_values().collect(),
    };
    metadata.canonicalize();
    Ok(metadata)
}

fn table_mut<'a>(
    schemas: &'a mut HashMap<String, HashMap<String, TableMetadata>>,
    schema_name: &str,
    table_name: &str,
) -> Option<&'a mut TableMetadata> {
    schemas
        .get_mut(schema_name)
        .and_then(|schema| schema.get_mut(table_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat catalog rows aggregate into one canonical metadata tree:
    /// tables grouped per schema, constraints/keys/indexes attached to
    /// their table, type operations merged per type.
    #[test]
    fn rows_aggregate_into_canonical_metadata() {
        let metadata = metadata_from_rows(
            vec![
                FlatColumn {
                    schema_name: "public".into(),
                    table_name: "title".into(),
                    table_description: Some("Localized title records.".into()),
                    column_name: "id".into(),
                    column_description: Some("Stable title identifier.".into()),
                    type_oid: 23,
                    type_modifier: -1,
                    formatted_type: "integer".into(),
                    not_null: true,
                    object_type: "r".into(),
                },
                FlatColumn {
                    schema_name: "public".into(),
                    table_name: "title".into(),
                    table_description: Some("Localized title records.".into()),
                    column_name: "kind_id".into(),
                    column_description: None,
                    type_oid: 23,
                    type_modifier: -1,
                    formatted_type: "integer".into(),
                    not_null: false,
                    object_type: "r".into(),
                },
                FlatColumn {
                    schema_name: "public".into(),
                    table_name: "kind_type".into(),
                    table_description: None,
                    column_name: "id".into(),
                    column_description: None,
                    type_oid: 23,
                    type_modifier: -1,
                    formatted_type: "integer".into(),
                    not_null: true,
                    object_type: "r".into(),
                },
            ],
            vec![PostgresConstraint {
                schema_name: "public".into(),
                table_name: "title".into(),
                constraint_name: "title_pkey".into(),
                constraint_kind: "primary_key".into(),
                columns: vec!["id".into()],
            }],
            vec![PostgresForeignKey {
                schema_name: "public".into(),
                table_name: "title".into(),
                foreign_key_name: "title_kind_id_fkey".into(),
                columns: vec!["kind_id".into()],
                referenced_schema_name: "public".into(),
                referenced_table_name: "kind_type".into(),
                referenced_columns: vec!["id".into()],
            }],
            vec![
                PostgresIndexKey {
                    schema_name: "public".into(),
                    table_name: "kind_type".into(),
                    index_name: "kind_type_incomplete_key".into(),
                    access_method: "btree".into(),
                    is_unique: true,
                    key_position: 1,
                    key_count: 2,
                    column_name: "id".into(),
                    operator_class: Some("pg_catalog.int4_ops".into()),
                    native_operators: vec!["=".into(), "<".into()],
                    orderable: Some(true),
                    ascending: Some(true),
                    nulls_first: Some(false),
                    included_columns: Vec::new(),
                },
                PostgresIndexKey {
                    schema_name: "public".into(),
                    table_name: "title".into(),
                    index_name: "title_pkey".into(),
                    access_method: "btree".into(),
                    is_unique: true,
                    key_position: 1,
                    key_count: 1,
                    column_name: "id".into(),
                    operator_class: Some("pg_catalog.int4_ops".into()),
                    native_operators: vec!["=".into(), "<".into()],
                    orderable: Some(true),
                    ascending: Some(true),
                    nulls_first: Some(false),
                    included_columns: vec!["kind_id".into()],
                },
            ],
            vec![
                PostgresType {
                    type_oid: 23,
                    internal_type: "int4".into(),
                    readable_type: "integer".into(),
                    type_schema: "pg_catalog".into(),
                    type_kind: "b".into(),
                    type_category: "N".into(),
                    operations: vec!["=".into(), "<".into()],
                    orderable: true,
                },
                PostgresType {
                    type_oid: 90_001,
                    internal_type: "person".into(),
                    readable_type: "alpha.person".into(),
                    type_schema: "alpha".into(),
                    type_kind: "e".into(),
                    type_category: "E".into(),
                    operations: vec![
                        "<".into(),
                        "<=".into(),
                        "<>".into(),
                        "=".into(),
                        ">".into(),
                        ">=".into(),
                    ],
                    orderable: true,
                },
                PostgresType {
                    type_oid: 90_002,
                    internal_type: "person".into(),
                    readable_type: "beta.person".into(),
                    type_schema: "beta".into(),
                    type_kind: "e".into(),
                    type_category: "E".into(),
                    operations: vec!["=".into()],
                    orderable: true,
                },
                PostgresType {
                    type_oid: 1_007,
                    internal_type: "_int4".into(),
                    readable_type: "integer[]".into(),
                    type_schema: "pg_catalog".into(),
                    type_kind: "b".into(),
                    type_category: "A".into(),
                    operations: vec!["<>".into(), "=".into()],
                    orderable: true,
                },
                PostgresType {
                    type_oid: 3_904,
                    internal_type: "int4range".into(),
                    readable_type: "int4range".into(),
                    type_schema: "pg_catalog".into(),
                    type_kind: "r".into(),
                    type_category: "R".into(),
                    operations: vec!["<".into(), "<>".into(), "=".into(), ">".into()],
                    orderable: true,
                },
                PostgresType {
                    type_oid: 4_451,
                    internal_type: "int4multirange".into(),
                    readable_type: "int4multirange".into(),
                    type_schema: "pg_catalog".into(),
                    type_kind: "m".into(),
                    type_category: "R".into(),
                    operations: vec!["<".into(), "<>".into(), "=".into(), ">".into()],
                    orderable: true,
                },
            ],
        )
        .expect("one catalog snapshot is complete");

        assert_eq!(metadata.schemas.len(), 1);
        let schema = &metadata.schemas[0];
        assert_eq!(schema.name, "public");
        let names: Vec<&str> = schema
            .tables
            .iter()
            .map(|table| table.name.as_str())
            .collect();
        assert_eq!(names, ["kind_type", "title"], "tables sort by name");
        assert!(
            schema.tables[0].indexes.is_empty(),
            "an index missing its final declared key is dropped entirely"
        );

        let title = &schema.tables[1];
        assert_eq!(
            title.description.as_deref(),
            Some("Localized title records.")
        );
        assert_eq!(title.columns.len(), 2);
        assert_eq!(
            title.columns[0].description.as_deref(),
            Some("Stable title identifier.")
        );
        assert_eq!(title.columns[0].formatted_type.as_deref(), Some("integer"));
        assert_eq!(title.columns[0].type_modifier, Some(-1));
        assert_eq!(title.constraints.len(), 1);
        assert_eq!(title.foreign_keys.len(), 1);
        assert_eq!(title.foreign_keys[0].references.table, "kind_type");
        assert_eq!(title.indexes.len(), 1);
        assert_eq!(title.indexes[0].access_method, "btree");
        assert_eq!(title.indexes[0].included_columns, ["kind_id"]);
        assert_eq!(title.indexes[0].keys.len(), 1);
        assert_eq!(
            title.indexes[0].keys[0].operator_class.as_deref(),
            Some("pg_catalog.int4_ops")
        );
        assert_eq!(
            title.indexes[0].keys[0].capabilities,
            [IndexKeyCapability::Equality, IndexKeyCapability::Range,]
        );
        assert_eq!(
            title.indexes[0].keys[0].order,
            Some(IndexOrder {
                direction: IndexOrderDirection::Asc,
                nulls: IndexNullsPosition::Last,
            })
        );

        assert_eq!(
            metadata
                .types
                .iter()
                .map(TypeMetadata::key)
                .collect::<Vec<_>>(),
            [
                TypeKey::new("alpha", "person"),
                TypeKey::new("beta", "person"),
                TypeKey::new("pg_catalog", "_int4"),
                TypeKey::new("pg_catalog", "int4"),
                TypeKey::new("pg_catalog", "int4multirange"),
                TypeKey::new("pg_catalog", "int4range"),
            ],
            "same-named types remain distinct and sort by qualified identity"
        );
        assert_eq!(
            metadata.types[0].operations,
            BTreeSet::from([
                "<".to_string(),
                "<=".to_string(),
                "<>".to_string(),
                "=".to_string(),
                ">".to_string(),
                ">=".to_string(),
            ]),
            "enum comparison facts survive polymorphic-family derivation"
        );
        assert_eq!(
            metadata.types[0].provider,
            Some(ProviderTypeFacts {
                kind: "e".to_string(),
                category: "E".to_string(),
                orderable: true,
            })
        );
        assert_eq!(
            metadata.types[3].provider,
            Some(ProviderTypeFacts {
                kind: "b".to_string(),
                category: "N".to_string(),
                orderable: true,
            })
        );
        let operations: Vec<&str> = metadata.types[3]
            .operations
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(operations, ["<", "="]);
        assert_eq!(
            metadata.types[2].provider,
            Some(ProviderTypeFacts {
                kind: "b".to_string(),
                category: "A".to_string(),
                orderable: true,
            }),
            "array capabilities survive the polymorphic anyarray family"
        );
        assert_eq!(
            metadata.types[4].provider,
            Some(ProviderTypeFacts {
                kind: "m".to_string(),
                category: "R".to_string(),
                orderable: true,
            }),
            "multirange capabilities survive the polymorphic family"
        );
        assert_eq!(
            metadata.types[5].provider,
            Some(ProviderTypeFacts {
                kind: "r".to_string(),
                category: "R".to_string(),
                orderable: true,
            }),
            "range capabilities survive the polymorphic family"
        );
        let catalog = metadata.to_catalog().expect("metadata builds a catalog");
        let kind_type = catalog
            .table("public", "kind_type")
            .expect("kind_type resolves");
        let kind_type_id = catalog
            .columns_for_table(kind_type.id)
            .find(|column| column.name == "id")
            .expect("kind_type.id resolves");
        assert!(
            !catalog.column_set_is_unique(kind_type.id, &[kind_type_id.id]),
            "an incomplete unique index cannot prove catalog uniqueness"
        );
    }

    /// Rows against tables the column pass never produced (e.g. filtered
    /// relkinds) are dropped rather than invented.
    #[test]
    fn rows_for_unknown_tables_are_dropped() {
        let metadata = metadata_from_rows(
            Vec::new(),
            vec![PostgresConstraint {
                schema_name: "public".into(),
                table_name: "ghost".into(),
                constraint_name: "ghost_pkey".into(),
                constraint_kind: "primary_key".into(),
                columns: vec!["id".into()],
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("an empty catalog snapshot is complete");
        assert!(metadata.schemas.is_empty());
    }

    #[test]
    fn unresolved_column_type_oids_fail_the_snapshot() {
        let error = metadata_from_rows(
            vec![FlatColumn {
                schema_name: "public".into(),
                table_name: "records".into(),
                table_description: None,
                column_name: "value".into(),
                column_description: None,
                type_oid: 90_001,
                type_modifier: -1,
                formatted_type: "public.value".into(),
                not_null: true,
                object_type: "r".into(),
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect_err("a column type must exist in the same snapshot");

        assert_eq!(
            error.to_string(),
            "column `public.records.value` references PostgreSQL type OID 90001, \
             but that type was absent from the same catalog snapshot"
        );
    }
}
