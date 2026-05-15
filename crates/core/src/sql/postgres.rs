use crate::{
    BinaryOp, Catalog, Column, FilterColumnScope, FilterExpr, FilterLiteral, ForeignKey, QueryPlan,
    SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan, Table,
};
use sea_query::{
    Alias, Asterisk, Condition, Expr, ExprTrait, Func, JoinType, Order, PgFunc,
    PostgresQueryBuilder, Query, SelectStatement,
};
use std::fmt::Write;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedSql {
    pub output_name: String,
    pub sql: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostgresSqlOptions {
    pub collection_limit: Option<u64>,
}

#[derive(Debug, Error)]
pub enum SqlGenerationError {
    #[error("table id `{0}` was not found in catalog")]
    MissingTable(usize),
    #[error("column id `{0}` was not found in catalog")]
    MissingColumn(usize),
    #[error("foreign key id `{0}` was not found in catalog")]
    MissingForeignKey(usize),
    #[error(
        "foreign key `{foreign_key}` does not connect parent table `{parent}` to child table `{child}`"
    )]
    InvalidRelation {
        foreign_key: usize,
        parent: String,
        child: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelationCardinality {
    Object,
    Collection,
}

#[derive(Clone, Debug)]
struct SelectionContext {
    table_alias: String,
    json_alias: String,
}

pub fn generate_postgres_sql(
    plan: &QueryPlan,
    catalog: &Catalog,
) -> Result<GeneratedSql, SqlGenerationError> {
    generate_postgres_sql_with_options(plan, catalog, PostgresSqlOptions::default())
}

pub fn generate_postgres_sql_with_options(
    plan: &QueryPlan,
    catalog: &Catalog,
    options: PostgresSqlOptions,
) -> Result<GeneratedSql, SqlGenerationError> {
    let mut path = Vec::new();
    let root = catalog
        .table_by_id(plan.root)
        .ok_or(SqlGenerationError::MissingTable(plan.root.0))?;
    path.push(path_segment(root, &plan.output_name));
    let root_query = generate_selection(
        &plan.selections,
        catalog,
        &plan.output_name,
        &path,
        None,
        None,
        RelationCardinality::Collection,
        options,
    )?;
    let format_options = sqlformat::FormatOptions {
        uppercase: Some(false),
        indent: sqlformat::Indent::Spaces(2),
        ..Default::default()
    };
    let sql = sqlformat::format(
        &root_query.to_string(PostgresQueryBuilder),
        &sqlformat::QueryParams::default(),
        &format_options,
    );
    Ok(GeneratedSql {
        output_name: plan.output_name.clone(),
        sql,
    })
}

fn generate_selection(
    selection: &SelectionPlan,
    catalog: &Catalog,
    output_name: &str,
    path: &[String],
    parent: Option<(&SelectionContext, &ForeignKey)>,
    root: Option<&SelectionContext>,
    cardinality: RelationCardinality,
    options: PostgresSqlOptions,
) -> Result<SelectStatement, SqlGenerationError> {
    let current_table = table(catalog, selection.table)?;
    let context = context_for(current_table, output_name, path);
    let root_context = root.unwrap_or(&context);
    let mut query = Query::select();

    let relation_condition = if let Some((parent, foreign_key)) = parent {
        Some(relation_condition(
            catalog,
            parent,
            &context,
            foreign_key,
            selection.table,
        )?)
    } else {
        None
    };
    let filter = selection
        .clauses
        .filter
        .as_ref()
        .map(|filter| filter_expr(catalog, &context, root_context, None, filter))
        .transpose()?;
    if cardinality == RelationCardinality::Collection
        && should_use_source_subquery(&selection.clauses, options)
    {
        let source = limited_source_query(
            catalog,
            current_table,
            &context,
            relation_condition,
            filter,
            &selection.clauses,
            effective_limit(&selection.clauses, options),
        )?;
        query.from_subquery(source, Alias::new(&context.table_alias));
    } else {
        query.from_as(
            (
                Alias::new(&current_table.schema),
                Alias::new(&current_table.name),
            ),
            Alias::new(&context.table_alias),
        );
        if let Some(relation_condition) = relation_condition {
            query.cond_where(relation_condition);
        }
        if let Some(filter) = filter {
            query.and_where(filter);
        }
        apply_order_limit_offset(catalog, &context, &selection.clauses, &mut query)?;
    };

    for item in &selection.items {
        let SelectionPlanItem::Relation(relation) = item else {
            continue;
        };
        let related_table = table(catalog, relation.table)?;
        let mut relation_path = path.to_vec();
        relation_path.push(path_segment(related_table, &relation.output_name));
        let foreign_key = foreign_key(catalog, relation.foreign_key)?;
        let relation_cardinality =
            relation_cardinality(selection.table, relation.table, foreign_key).ok_or_else(
                || SqlGenerationError::InvalidRelation {
                    foreign_key: relation.foreign_key.0,
                    parent: table_label(current_table),
                    child: table_label(related_table),
                },
            )?;
        let child_context = context_for(related_table, &relation.output_name, &relation_path);
        let child_query = generate_selection(
            &relation.selections,
            catalog,
            &relation.output_name,
            &relation_path,
            Some((&context, foreign_key)),
            Some(root_context),
            relation_cardinality,
            options,
        )?;
        query.join_lateral(
            JoinType::LeftJoin,
            child_query,
            Alias::new(&child_context.json_alias),
            Expr::cust("true"),
        );
    }

    let object = json_build_object(selection, catalog, &context, path)?;
    let expression = match cardinality {
        RelationCardinality::Object => object,
        RelationCardinality::Collection => {
            Func::coalesce([PgFunc::json_agg(object).into(), Expr::value("[]")]).into()
        }
    };
    query.expr_as(expression, Alias::new(output_name));
    Ok(query.to_owned())
}

fn limited_source_query(
    catalog: &Catalog,
    table: &Table,
    context: &SelectionContext,
    relation_condition: Option<Condition>,
    filter: Option<Expr>,
    clauses: &SelectionClauses,
    limit: Option<u64>,
) -> Result<SelectStatement, SqlGenerationError> {
    let mut query = Query::select();
    query.column(Asterisk).from_as(
        (Alias::new(&table.schema), Alias::new(&table.name)),
        Alias::new(&context.table_alias),
    );
    if let Some(limit) = limit {
        query.limit(limit);
    }
    if let Some(relation_condition) = relation_condition {
        query.cond_where(relation_condition);
    }
    if let Some(filter) = filter {
        query.and_where(filter);
    }
    for order in &clauses.order_by {
        let column = column(catalog, order.column)?;
        query.order_by(
            (Alias::new(&context.table_alias), Alias::new(&column.name)),
            match order.direction {
                SortDirectionPlan::Asc => Order::Asc,
                SortDirectionPlan::Desc => Order::Desc,
            },
        );
    }
    if let Some(offset) = clauses.offset {
        query.offset(offset);
    }
    Ok(query.to_owned())
}

fn effective_limit(clauses: &SelectionClauses, options: PostgresSqlOptions) -> Option<u64> {
    match (clauses.limit, options.collection_limit) {
        (Some(source), Some(guard)) => Some(std::cmp::Ord::min(source, guard)),
        (Some(source), None) => Some(source),
        (None, Some(guard)) => Some(guard),
        (None, None) => None,
    }
}

fn should_use_source_subquery(clauses: &SelectionClauses, options: PostgresSqlOptions) -> bool {
    effective_limit(clauses, options).is_some()
        || clauses.offset.is_some()
        || !clauses.order_by.is_empty()
}

fn apply_order_limit_offset(
    catalog: &Catalog,
    context: &SelectionContext,
    clauses: &SelectionClauses,
    query: &mut SelectStatement,
) -> Result<(), SqlGenerationError> {
    for order in &clauses.order_by {
        let column = column(catalog, order.column)?;
        query.order_by(
            (Alias::new(&context.table_alias), Alias::new(&column.name)),
            match order.direction {
                SortDirectionPlan::Asc => Order::Asc,
                SortDirectionPlan::Desc => Order::Desc,
            },
        );
    }
    if let Some(limit) = clauses.limit {
        query.limit(limit);
    }
    if let Some(offset) = clauses.offset {
        query.offset(offset);
    }
    Ok(())
}

fn filter_expr(
    catalog: &Catalog,
    context: &SelectionContext,
    root: &SelectionContext,
    outer_current: Option<&SelectionContext>,
    filter: &FilterExpr,
) -> Result<Expr, SqlGenerationError> {
    Ok(match filter {
        FilterExpr::Column {
            scope,
            column: column_id,
        } => {
            let column = column(catalog, *column_id)?;
            let source = match scope {
                FilterColumnScope::Current => context,
                FilterColumnScope::Root => root,
                FilterColumnScope::OuterCurrent => outer_current.unwrap_or(context),
            };
            Expr::col((Alias::new(&source.table_alias), Alias::new(&column.name)))
        }
        FilterExpr::Literal(literal) => match literal {
            FilterLiteral::String(value) => Expr::value(value.clone()),
            FilterLiteral::Number(value) => value
                .parse::<i64>()
                .map(Expr::value)
                .or_else(|_| value.parse::<f64>().map(Expr::value))
                .unwrap_or_else(|_| Expr::value(value.clone())),
            FilterLiteral::Bool(value) => Expr::value(*value),
            FilterLiteral::Null => Expr::cust("null"),
        },
        FilterExpr::Binary { left, op, right } => {
            if *op == BinaryOp::Like
                && let FilterExpr::Literal(FilterLiteral::String(pattern)) = right.as_ref()
            {
                let left = filter_expr(catalog, context, root, outer_current, left)?;
                return Ok(left.like(pattern.clone()));
            }
            let left = filter_expr(catalog, context, root, outer_current, left)?;
            let right = filter_expr(catalog, context, root, outer_current, right)?;
            match op {
                BinaryOp::Eq => left.eq(right),
                BinaryOp::Ne => left.ne(right),
                BinaryOp::Gt => left.gt(right),
                BinaryOp::Ge => left.gte(right),
                BinaryOp::Lt => left.lt(right),
                BinaryOp::Le => left.lte(right),
                BinaryOp::Like => left.like("<unsupported>"),
                BinaryOp::And => left.and(right),
                BinaryOp::Or => left.or(right),
            }
        }
        FilterExpr::Exists {
            foreign_key: foreign_key_id,
            table: table_id,
            filter,
        } => {
            let related_table = table(catalog, *table_id)?;
            let foreign_key = foreign_key(catalog, *foreign_key_id)?;
            let exists_context = context_for(
                related_table,
                &related_table.name,
                &[table_label(related_table)],
            );
            let mut query = Query::select();
            query.expr(Expr::value(1)).from_as(
                (
                    Alias::new(&related_table.schema),
                    Alias::new(&related_table.name),
                ),
                Alias::new(&exists_context.table_alias),
            );
            query.cond_where(relation_condition(
                catalog,
                context,
                &exists_context,
                foreign_key,
                *table_id,
            )?);
            query.and_where(filter_expr(
                catalog,
                &exists_context,
                root,
                outer_current.or(Some(context)),
                filter,
            )?);
            Expr::exists(query.to_owned())
        }
    })
}

fn json_build_object(
    selection: &SelectionPlan,
    catalog: &Catalog,
    context: &SelectionContext,
    path: &[String],
) -> Result<Expr, SqlGenerationError> {
    let mut pairs = Vec::new();
    for item in &selection.items {
        match item {
            SelectionPlanItem::Projection(projection) => {
                let column = column(catalog, projection.column)?;
                pairs.push((
                    Expr::value(projection.output_name.clone()),
                    Expr::col((Alias::new(&context.table_alias), Alias::new(&column.name))),
                ));
            }
            SelectionPlanItem::Relation(relation) => {
                let table = table(catalog, relation.table)?;
                let mut relation_path = path.to_vec();
                relation_path.push(path_segment(table, &relation.output_name));
                let related_context = context_for(table, &relation.output_name, &relation_path);
                pairs.push((
                    Expr::value(relation.output_name.clone()),
                    Expr::col((
                        Alias::new(&related_context.json_alias),
                        Alias::new(&relation.output_name),
                    )),
                ));
            }
        }
    }
    Ok(PgFunc::json_build_object(pairs).into())
}

fn relation_condition(
    catalog: &Catalog,
    parent: &SelectionContext,
    child: &SelectionContext,
    foreign_key: &ForeignKey,
    child_table: crate::TableId,
) -> Result<Condition, SqlGenerationError> {
    let mut condition = Condition::all();
    if child_table == foreign_key.from_table {
        for (from_column, to_column) in foreign_key
            .from_columns
            .iter()
            .zip(foreign_key.to_columns.iter())
        {
            condition = condition.add(
                Expr::col((
                    Alias::new(&child.table_alias),
                    Alias::new(&column(catalog, *from_column)?.name),
                ))
                .equals((
                    Alias::new(&parent.table_alias),
                    Alias::new(&column(catalog, *to_column)?.name),
                )),
            );
        }
    } else {
        for (from_column, to_column) in foreign_key
            .from_columns
            .iter()
            .zip(foreign_key.to_columns.iter())
        {
            condition = condition.add(
                Expr::col((
                    Alias::new(&child.table_alias),
                    Alias::new(&column(catalog, *to_column)?.name),
                ))
                .equals((
                    Alias::new(&parent.table_alias),
                    Alias::new(&column(catalog, *from_column)?.name),
                )),
            );
        }
    }
    Ok(condition)
}

fn relation_cardinality(
    parent: crate::TableId,
    child: crate::TableId,
    foreign_key: &ForeignKey,
) -> Option<RelationCardinality> {
    if parent == foreign_key.to_table && child == foreign_key.from_table {
        Some(RelationCardinality::Collection)
    } else if parent == foreign_key.from_table && child == foreign_key.to_table {
        Some(RelationCardinality::Object)
    } else {
        None
    }
}

fn context_for(table: &Table, output_name: &str, path: &[String]) -> SelectionContext {
    let suffix = short_hash(&path.join("/"));
    let base = sanitize_alias(output_name)
        .or_else(|| sanitize_alias(&table.name))
        .unwrap_or_else(|| "selection".to_string());
    SelectionContext {
        table_alias: format!("{base}_{suffix}"),
        json_alias: format!("{base}_json_{suffix}"),
    }
}

fn path_segment(table: &Table, output_name: &str) -> String {
    format!("{}.{}:{output_name}", table.schema, table.name)
}

fn sanitize_alias(value: &str) -> Option<String> {
    let mut alias = String::new();
    let mut last_was_underscore = false;
    for character in value.chars() {
        let mapped = if character.is_ascii_alphanumeric() {
            Some(character.to_ascii_lowercase())
        } else if character == '_' {
            Some('_')
        } else {
            None
        };
        if let Some(character) = mapped {
            if character == '_' && last_was_underscore {
                continue;
            }
            last_was_underscore = character == '_';
            alias.push(character);
        } else if !last_was_underscore && !alias.is_empty() {
            alias.push('_');
            last_was_underscore = true;
        }
        if alias.len() >= 24 {
            break;
        }
    }
    let alias = alias.trim_matches('_').to_string();
    (!alias.is_empty()).then_some(alias)
}

fn short_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut output = String::new();
    write!(&mut output, "{:08x}", hash as u32).expect("write hash");
    output
}

fn table(catalog: &Catalog, id: crate::TableId) -> Result<&Table, SqlGenerationError> {
    catalog
        .table_by_id(id)
        .ok_or(SqlGenerationError::MissingTable(id.0))
}

fn column(catalog: &Catalog, id: crate::ColumnId) -> Result<&Column, SqlGenerationError> {
    catalog
        .column_by_id(id)
        .ok_or(SqlGenerationError::MissingColumn(id.0))
}

fn foreign_key(
    catalog: &Catalog,
    id: crate::ForeignKeyId,
) -> Result<&ForeignKey, SqlGenerationError> {
    catalog
        .foreign_key_by_id(id)
        .ok_or(SqlGenerationError::MissingForeignKey(id.0))
}

fn table_label(table: &Table) -> String {
    format!("{}.{}", table.schema, table.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Catalog, parse_source, plan_file_with_catalog};

    #[test]
    fn generates_nested_json_sql_shape() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source("query Q { users { id name posts { title } } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();
        let sql = generated.sql.to_ascii_lowercase();

        assert_eq!(generated.output_name, "users");
        assert!(sql.contains("json_build_object"));
        assert!(sql.contains("json_agg"));
        assert!(sql.contains("left join lateral"));
        assert!(sql.contains("\"public\".\"users\""));
        assert!(sql.contains("\"public\".\"posts\""));
        assert!(sql.contains("'posts'"));
        assert!(sql.contains("'[]'"));
    }

    #[test]
    fn generates_clause_sql_shape() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source(
            "query Q { posts(where .id > 100 order by created_at desc limit 10 offset 5) { id title } }"
                .into(),
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();
        let sql = generated.sql.to_ascii_lowercase();

        assert!(sql.contains("where"));
        assert!(sql.contains("\"posts_"));
        assert!(sql.contains("\"id\" > 100"));
        assert!(sql.contains("order by"));
        assert!(sql.contains("\"created_at\" desc"));
        assert!(sql.contains("limit"));
        assert!(sql.contains("offset"));
    }

    #[test]
    fn generates_scoped_relationship_predicate_sql_shape() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source(
            "query Q { users(where .posts.title like \"%foo%\") { id posts(where .user_id == ~id) { id } } }"
                .into(),
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();
        let sql = generated.sql.to_ascii_lowercase();

        assert!(sql.contains("exists"));
        assert!(sql.contains("like"));
        assert!(sql.contains("\"title\" like '%foo%'"));
        assert!(sql.contains("\"user_id\" = \"users_"));
    }
}
