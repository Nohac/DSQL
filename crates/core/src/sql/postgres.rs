use crate::{
    BinaryOp, Catalog, Column, FilterColumnScope, FilterExpr, FilterLiteral, ForeignKey, QueryPlan,
    RelationCardinality, SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan,
    SqlParameter, SqlValue, SqlVariantCase, Table,
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
    pub parameters: Vec<GeneratedSqlParameter>,
    pub variants: Vec<GeneratedSqlVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedSqlParameter {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedSqlVariant {
    pub path: String,
    pub cases: Vec<SqlVariantCase>,
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

#[derive(Clone, Debug)]
struct SelectionContext {
    table_alias: String,
    json_alias: String,
    result_alias: String,
}

struct SelectionGenerationContext<'a> {
    parent: Option<(&'a SelectionContext, &'a ForeignKey)>,
    root: Option<&'a SelectionContext>,
    cardinality: RelationCardinality,
    options: PostgresSqlOptions,
    public_result_alias: Option<&'a str>,
}

struct SqlTemplateContext {
    parameters: Vec<GeneratedSqlParameter>,
    variants: Vec<GeneratedSqlVariant>,
    replacements: Vec<(String, String)>,
    next_sentinel: u64,
}

impl Default for SqlTemplateContext {
    fn default() -> Self {
        Self {
            parameters: Vec::new(),
            variants: Vec::new(),
            replacements: Vec::new(),
            next_sentinel: 9_000_000_000_000_000_000,
        }
    }
}

impl SqlTemplateContext {
    fn parameter(&mut self, parameter: &SqlParameter) -> String {
        if let Some(index) = self
            .parameters
            .iter()
            .position(|candidate| candidate.path == parameter.path)
        {
            return format!("${}", index + 1);
        }
        self.parameters.push(GeneratedSqlParameter {
            path: parameter.path.clone(),
        });
        format!("${}", self.parameters.len())
    }

    fn variant(&mut self, path: &str, cases: &[SqlVariantCase]) -> String {
        if !self.variants.iter().any(|variant| variant.path == path) {
            self.variants.push(GeneratedSqlVariant {
                path: path.to_string(),
                cases: cases.to_vec(),
            });
        }
        self.replacements
            .push((format!("{{ {{ {path} }} }}"), format!("{{{{{path}}}}}")));
        format!("{{{{{path}}}}}")
    }

    fn replace_order_direction(
        &mut self,
        table_alias: &str,
        column: &str,
        path: &str,
        cases: &[SqlVariantCase],
    ) {
        let placeholder = self.variant(path, cases);
        self.replacements.push((
            format!("\"{table_alias}\".\"{column}\" asc"),
            format!("\"{table_alias}\".\"{column}\" {placeholder}"),
        ));
    }

    fn numeric_parameter_sentinel(&mut self, parameter: &SqlParameter) -> u64 {
        let placeholder = self.parameter(parameter);
        let sentinel = self.next_sentinel;
        self.next_sentinel += 1;
        self.replacements.push((sentinel.to_string(), placeholder));
        sentinel
    }
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
    let mut template = SqlTemplateContext::default();
    let root = catalog
        .table_by_id(plan.root)
        .ok_or(SqlGenerationError::MissingTable(plan.root.0))?;
    path.push(path_segment(root, &plan.output_name));
    let root_query = generate_selection(
        &plan.selections,
        catalog,
        &plan.output_name,
        &path,
        SelectionGenerationContext {
            parent: None,
            root: None,
            cardinality: RelationCardinality::Collection,
            options,
            public_result_alias: Some(&plan.output_name),
        },
        &mut template,
    )?;
    let format_options = sqlformat::FormatOptions {
        uppercase: Some(false),
        indent: sqlformat::Indent::Spaces(2),
        ..Default::default()
    };
    let mut sql = sqlformat::format(
        &root_query.to_string(PostgresQueryBuilder),
        &sqlformat::QueryParams::default(),
        &format_options,
    );
    for (needle, replacement) in &template.replacements {
        sql = sql.replace(needle, replacement);
    }
    Ok(GeneratedSql {
        output_name: plan.output_name.clone(),
        sql,
        parameters: template.parameters,
        variants: template.variants,
    })
}

fn generate_selection(
    selection: &SelectionPlan,
    catalog: &Catalog,
    output_name: &str,
    path: &[String],
    generation: SelectionGenerationContext<'_>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    let current_table = table(catalog, selection.table)?;
    let context = context_for(current_table, output_name, path);
    let root_context = generation.root.unwrap_or(&context);
    let mut query = Query::select();

    let relation_condition = if let Some((parent, foreign_key)) = generation.parent {
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
        .map(|filter| filter_expr(catalog, &context, root_context, None, filter, template))
        .transpose()?;
    if generation.cardinality == RelationCardinality::Collection
        && should_use_source_subquery(&selection.clauses, generation.options)
    {
        let source = limited_source_query(
            catalog,
            current_table,
            &context,
            relation_condition,
            filter,
            &selection.clauses,
            effective_limit(&selection.clauses, generation.options),
            template,
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
        apply_order_limit_offset(catalog, &context, &selection.clauses, &mut query, template)?;
    };

    for item in &selection.items {
        let SelectionPlanItem::Relation(relation) = item else {
            continue;
        };
        let related_table = table(catalog, relation.table)?;
        let mut relation_path = path.to_vec();
        relation_path.push(path_segment(related_table, &relation.output_name));
        let foreign_key = foreign_key(catalog, relation.foreign_key)?;
        let relation_cardinality = catalog
            .relation_cardinality(selection.table, relation.table, foreign_key)
            .ok_or_else(|| SqlGenerationError::InvalidRelation {
                foreign_key: relation.foreign_key.0,
                parent: table_label(current_table),
                child: table_label(related_table),
            })?;
        let child_context = context_for(related_table, &relation.output_name, &relation_path);
        let child_query = generate_selection(
            &relation.selections,
            catalog,
            &relation.output_name,
            &relation_path,
            SelectionGenerationContext {
                parent: Some((&context, foreign_key)),
                root: Some(root_context),
                cardinality: relation_cardinality,
                options: generation.options,
                public_result_alias: None,
            },
            template,
        )?;
        query.join_lateral(
            JoinType::LeftJoin,
            child_query,
            Alias::new(&child_context.json_alias),
            Expr::cust("true"),
        );
    }

    let object = json_build_object(selection, catalog, &context, path)?;
    let expression: Expr = match generation.cardinality {
        RelationCardinality::Collection => {
            Func::coalesce([PgFunc::json_agg(object).into(), Expr::value("[]")]).into()
        }
        RelationCardinality::Singular => object,
    };
    query.expr_as(
        expression,
        Alias::new(
            generation
                .public_result_alias
                .unwrap_or(&context.result_alias),
        ),
    );
    Ok(query.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn limited_source_query(
    catalog: &Catalog,
    table: &Table,
    context: &SelectionContext,
    relation_condition: Option<Condition>,
    filter: Option<Expr>,
    clauses: &SelectionClauses,
    limit: Option<u64>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    let mut query = Query::select();
    query.column(Asterisk).from_as(
        (Alias::new(&table.schema), Alias::new(&table.name)),
        Alias::new(&context.table_alias),
    );
    if let Some(limit) = limit {
        query.limit(limit);
    } else if let Some(SqlValue::Parameter(parameter)) = &clauses.limit {
        query.limit(template.numeric_parameter_sentinel(parameter));
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
            match &order.direction {
                SortDirectionPlan::Asc => Order::Asc,
                SortDirectionPlan::Desc => Order::Desc,
                SortDirectionPlan::Variant { path, variants } => {
                    template.replace_order_direction(
                        &context.table_alias,
                        &column.name,
                        path,
                        variants,
                    );
                    Order::Asc
                }
            },
        );
    }
    if let Some(offset) = sql_value_u64(&clauses.offset) {
        query.offset(offset);
    } else if let Some(SqlValue::Parameter(parameter)) = &clauses.offset {
        query.offset(template.numeric_parameter_sentinel(parameter));
    }
    Ok(query.to_owned())
}

fn effective_limit(clauses: &SelectionClauses, options: PostgresSqlOptions) -> Option<u64> {
    match (sql_value_u64(&clauses.limit), options.collection_limit) {
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
    template: &mut SqlTemplateContext,
) -> Result<(), SqlGenerationError> {
    for order in &clauses.order_by {
        let column = column(catalog, order.column)?;
        query.order_by(
            (Alias::new(&context.table_alias), Alias::new(&column.name)),
            match &order.direction {
                SortDirectionPlan::Asc => Order::Asc,
                SortDirectionPlan::Desc => Order::Desc,
                SortDirectionPlan::Variant { path, variants } => {
                    template.replace_order_direction(
                        &context.table_alias,
                        &column.name,
                        path,
                        variants,
                    );
                    Order::Asc
                }
            },
        );
    }
    if let Some(limit) = sql_value_u64(&clauses.limit) {
        query.limit(limit);
    } else if let Some(SqlValue::Parameter(parameter)) = &clauses.limit {
        query.limit(template.numeric_parameter_sentinel(parameter));
    }
    if let Some(offset) = sql_value_u64(&clauses.offset) {
        query.offset(offset);
    } else if let Some(SqlValue::Parameter(parameter)) = &clauses.offset {
        query.offset(template.numeric_parameter_sentinel(parameter));
    }
    Ok(())
}

fn sql_value_u64(value: &Option<SqlValue>) -> Option<u64> {
    match value {
        Some(SqlValue::Literal(value)) => Some(*value),
        _ => None,
    }
}

fn filter_expr(
    catalog: &Catalog,
    context: &SelectionContext,
    root: &SelectionContext,
    outer_current: Option<&SelectionContext>,
    filter: &FilterExpr,
    template: &mut SqlTemplateContext,
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
        FilterExpr::Parameter(parameter) => Expr::cust(template.parameter(parameter)),
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
            if *op == BinaryOp::Like {
                return Ok(Expr::cust(format!(
                    "{} like {}",
                    filter_expr_fragment(catalog, context, root, outer_current, left, template)?,
                    filter_expr_fragment(catalog, context, root, outer_current, right, template)?
                )));
            }
            let left = filter_expr(catalog, context, root, outer_current, left, template)?;
            let right = filter_expr(catalog, context, root, outer_current, right, template)?;
            match op {
                BinaryOp::Eq => left.eq(right),
                BinaryOp::Ne => left.ne(right),
                BinaryOp::Gt => left.gt(right),
                BinaryOp::Ge => left.gte(right),
                BinaryOp::Lt => left.lt(right),
                BinaryOp::Le => left.lte(right),
                BinaryOp::Like => unreachable!("handled before generic binary expression lowering"),
                BinaryOp::And => left.and(right),
                BinaryOp::Or => left.or(right),
            }
        }
        FilterExpr::VariantBinary {
            left,
            path,
            variants,
            right,
        } => Expr::cust(format!(
            "{} {} {}",
            filter_expr_fragment(catalog, context, root, outer_current, left, template)?,
            template.variant(path, variants),
            filter_expr_fragment(catalog, context, root, outer_current, right, template)?
        )),
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
                template,
            )?);
            Expr::exists(query.to_owned())
        }
    })
}

fn filter_expr_fragment(
    catalog: &Catalog,
    context: &SelectionContext,
    root: &SelectionContext,
    outer_current: Option<&SelectionContext>,
    filter: &FilterExpr,
    template: &mut SqlTemplateContext,
) -> Result<String, SqlGenerationError> {
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
            format!("\"{}\".\"{}\"", source.table_alias, column.name)
        }
        FilterExpr::Parameter(parameter) => template.parameter(parameter),
        FilterExpr::Literal(FilterLiteral::String(value)) => sql_string(value),
        FilterExpr::Literal(FilterLiteral::Number(value)) => value.clone(),
        FilterExpr::Literal(FilterLiteral::Bool(value)) => value.to_string(),
        FilterExpr::Literal(FilterLiteral::Null) => "null".to_string(),
        other => {
            let _ = filter_expr(catalog, context, root, outer_current, other, template)?;
            "<unsupported>".to_string()
        }
    })
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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
                        Alias::new(&related_context.result_alias),
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

fn context_for(table: &Table, output_name: &str, path: &[String]) -> SelectionContext {
    let suffix = short_hash(&path.join("/"));
    let base = sanitize_alias(output_name)
        .or_else(|| sanitize_alias(&table.name))
        .unwrap_or_else(|| "selection".to_string());
    SelectionContext {
        table_alias: generated_identifier(&base, "_", &suffix),
        json_alias: generated_identifier(&base, "_json_", &suffix),
        result_alias: generated_identifier(&base, "_result_", &suffix),
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
    }
    let alias = alias.trim_matches('_').to_string();
    (!alias.is_empty()).then_some(alias)
}

const POSTGRES_IDENTIFIER_MAX_BYTES: usize = 63;

fn generated_identifier(base: &str, infix: &str, suffix: &str) -> String {
    let max_base_bytes = POSTGRES_IDENTIFIER_MAX_BYTES
        .saturating_sub(infix.len())
        .saturating_sub(suffix.len());
    let mut truncated_base = truncate_identifier_base(base, max_base_bytes);
    if truncated_base.is_empty() {
        truncated_base = truncate_identifier_base("selection", max_base_bytes);
    }
    format!("{truncated_base}{infix}{suffix}")
}

fn truncate_identifier_base(value: &str, max_bytes: usize) -> String {
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    value[..end].trim_matches('_').to_string()
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
    fn aliases_root_result_column_to_default_public_output_key() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source("query Q { users { id name } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert!(generated.sql.contains("as \"users\""), "{}", generated.sql);
    }

    #[test]
    fn aliases_root_result_column_to_user_public_output_key() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source("query Q { featured_users: users { id name } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert_eq!(generated.output_name, "featured_users");
        assert!(
            generated.sql.contains("as \"featured_users\""),
            "{}",
            generated.sql
        );
    }

    #[test]
    fn generates_parameterized_sql_and_operator_variants() {
        let catalog = Catalog::hardcoded();
        let parsed =
            parse_source("query Q { posts(where .id $[>, >=] $ limit $$) { id title } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert!(
            generated.sql.contains("{{input.posts.clause.where.id.op}}"),
            "{}",
            generated.sql
        );
        assert!(generated.sql.contains("$1"), "{}", generated.sql);
        assert!(generated.sql.contains("$2"), "{}", generated.sql);
        assert_eq!(
            generated
                .parameters
                .iter()
                .map(|parameter| parameter.path.as_str())
                .collect::<Vec<_>>(),
            vec!["input.posts.clause.where.id.value", "params.limit"]
        );
        assert_eq!(generated.variants.len(), 1);
        assert_eq!(generated.variants[0].path, "input.posts.clause.where.id.op");
        assert_eq!(
            generated.variants[0]
                .cases
                .iter()
                .map(|case| (case.value.as_str(), case.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(">", ">"), (">=", ">=")]
        );
    }

    #[test]
    fn generates_parameterized_like_sql() {
        let catalog = Catalog::hardcoded();
        let parsed =
            parse_source("query Q { posts(where .title like $$search) { id title } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert!(
            generated.sql.contains("\"title\" like $1"),
            "{}",
            generated.sql
        );
        assert!(
            !generated.sql.contains("<unsupported>"),
            "{}",
            generated.sql
        );
        assert_eq!(
            generated
                .parameters
                .iter()
                .map(|parameter| parameter.path.as_str())
                .collect::<Vec<_>>(),
            vec!["params.search"]
        );
    }

    #[test]
    fn generated_sql_identifiers_stay_within_postgres_limit() {
        let catalog = Catalog::hardcoded();
        let long_alias = "this_alias_name_is_long_but_under_postgres_identifier_limit";
        let parsed = parse_source(
            format!("query Q {{ {long_alias}: users {{ id {long_alias}: posts {{ title }} }} }}")
                .into(),
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert_eq!(generated.output_name, long_alias);
        assert!(
            generated.sql.contains(&format!("'{long_alias}'")),
            "{}",
            generated.sql
        );
        for identifier in quoted_identifiers(&generated.sql) {
            assert!(
                identifier.len() <= POSTGRES_IDENTIFIER_MAX_BYTES,
                "identifier `{identifier}` is {} bytes in:\n{}",
                identifier.len(),
                generated.sql
            );
        }
    }

    #[test]
    fn generated_context_aliases_are_bounded_and_hash_suffixed() {
        let catalog = Catalog::hardcoded();
        let table = catalog.table("public", "users").unwrap();
        let context = context_for(
            table,
            "this_alias_name_is_far_longer_than_postgresql_allows_for_identifiers_and_should_shrink",
            &["public.users:this_alias_name_is_far_longer_than_postgresql_allows_for_identifiers_and_should_shrink".to_string()],
        );

        for alias in [
            context.table_alias,
            context.json_alias,
            context.result_alias,
        ] {
            assert!(alias.len() <= POSTGRES_IDENTIFIER_MAX_BYTES, "{alias}");
            assert_eq!(alias.rsplit('_').next().unwrap().len(), 8, "{alias}");
        }
    }

    fn quoted_identifiers(sql: &str) -> Vec<&str> {
        sql.split('"').skip(1).step_by(2).collect()
    }

    #[test]
    fn generates_parameter_paths_for_relation_predicates() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source("query Q { users(where .posts.title == $) { id name } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert!(generated.sql.contains("$1"), "{}", generated.sql);
        assert_eq!(
            generated
                .parameters
                .iter()
                .map(|parameter| parameter.path.as_str())
                .collect::<Vec<_>>(),
            vec!["input.users.clause.where.posts.title"]
        );
    }

    #[test]
    fn generates_parameter_paths_for_nested_selection_clauses() {
        let catalog = Catalog::hardcoded();
        let parsed = parse_source("query Q { users { id posts(limit $) { id title } } }".into());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let planned = plan_file_with_catalog(&parsed.source_file, &catalog);
        assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);

        let generated = generate_postgres_sql(&planned.queries[0], &catalog).unwrap();

        assert!(generated.sql.contains("$1"), "{}", generated.sql);
        assert_eq!(
            generated
                .parameters
                .iter()
                .map(|parameter| parameter.path.as_str())
                .collect::<Vec<_>>(),
            vec!["input.users.body.posts.clause.limit"]
        );
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
