use crate::catalog::{
    Catalog, Column, ColumnId, DataType, ForeignKey, ForeignKeyId, Table, TableId,
};
use crate::entities::aggregate::{AggregateFunction, AggregateMode};
use crate::plan::{
    AggregatePlan, CollectionPlan, CollectionResultPlan, ExistsKind, FilterCollection,
    FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, NestedRelation, QueryPlan,
    SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan, SqlParameter, SqlValue,
    SqlVariantCase,
};
use crate::resolution::SelectionCardinality;
use sea_query::{
    Alias, Asterisk, Condition, Expr, ExprTrait, Func, JoinType, Order, PgFunc,
    PostgresQueryBuilder, Query, SelectStatement,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedSql {
    pub output_name: String,
    pub sql: String,
    pub parameters: Vec<GeneratedSqlParameter>,
    pub variants: Vec<GeneratedSqlVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedSqlParameter {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    #[error("aggregate function requires a planned operand")]
    MissingAggregateOperand,
    #[error("filter shape is not supported in a text fragment position")]
    UnsupportedFilterFragment,
    #[error("SQL template numeric sentinel space was exhausted")]
    TemplateSentinelExhausted,
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
    options: PostgresSqlOptions,
    public_result_alias: Option<&'a str>,
    flattened: bool,
}

struct SqlTemplateContext {
    parameters: Vec<GeneratedSqlParameter>,
    variants: Vec<GeneratedSqlVariant>,
    replacements: Vec<(String, String)>,
    numeric_replacements: Vec<(String, String)>,
    next_sentinel: u64,
}

const FIRST_NUMERIC_SENTINEL: u64 = 9_000_000_000_000_000_000;

impl SqlTemplateContext {
    fn new(next_sentinel: u64) -> Self {
        Self {
            parameters: Vec::new(),
            variants: Vec::new(),
            replacements: Vec::new(),
            numeric_replacements: Vec::new(),
            next_sentinel,
        }
    }

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

    fn numeric_parameter_sentinel(
        &mut self,
        parameter: &SqlParameter,
    ) -> Result<u64, SqlGenerationError> {
        let placeholder = self.parameter(parameter);
        self.numeric_sentinel(placeholder)
    }

    fn numeric_literal_sentinel(&mut self, literal: &str) -> Result<u64, SqlGenerationError> {
        self.numeric_sentinel(literal.to_string())
    }

    fn numeric_sentinel(&mut self, replacement: String) -> Result<u64, SqlGenerationError> {
        let sentinel = self.next_sentinel;
        self.next_sentinel = sentinel
            .checked_add(1)
            .ok_or(SqlGenerationError::TemplateSentinelExhausted)?;
        self.numeric_replacements
            .push((sentinel.to_string(), replacement));
        Ok(sentinel)
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
    let format_options = sqlformat::FormatOptions {
        uppercase: Some(false),
        indent: sqlformat::Indent::Spaces(2),
        ..Default::default()
    };
    let mut next_sentinel = FIRST_NUMERIC_SENTINEL;
    loop {
        let mut path = Vec::new();
        let mut template = SqlTemplateContext::new(next_sentinel);
        let root = table(catalog, plan.collection.table)?;
        path.push(path_segment(root, &plan.output_name));
        let root_query = generate_collection(
            &plan.collection,
            catalog,
            &plan.output_name,
            &path,
            SelectionGenerationContext {
                parent: None,
                root: None,
                options,
                public_result_alias: Some(&plan.output_name),
                flattened: plan.flattened,
            },
            &mut template,
        )?;
        let root_query = if matches!(plan.collection.result, CollectionResultPlan::Rows(_))
            && plan.collection.shape.cardinality == SelectionCardinality::AtMostOne
        {
            singular_root_envelope(plan, root_query)
        } else {
            root_query
        };
        let formatted = sqlformat::format(
            &root_query.to_string(PostgresQueryBuilder),
            &sqlformat::QueryParams::default(),
            &format_options,
        );
        let Some(mut sql) = replace_numeric_sentinels(&formatted, &template.numeric_replacements)
        else {
            next_sentinel = template.next_sentinel;
            continue;
        };
        for (needle, replacement) in &template.replacements {
            sql = sql.replace(needle, replacement);
        }
        return Ok(GeneratedSql {
            output_name: plan.output_name.clone(),
            sql,
            parameters: template.parameters,
            variants: template.variants,
        });
    }
}

fn singular_root_envelope(plan: &QueryPlan, root_query: SelectStatement) -> SelectStatement {
    const SINGLETON_ALIAS: &str = "dsql_singleton";
    const ROOT_ALIAS: &str = "dsql_root";

    let mut singleton = Query::select();
    singleton.expr(Expr::value(1));

    let mut output_names = if plan.flattened {
        let mut names = Vec::new();
        collect_collection_output_names(&plan.collection.result, &mut names);
        names
    } else {
        vec![plan.output_name.clone()]
    };
    output_names.dedup();

    let mut envelope = Query::select();
    for output_name in output_names {
        envelope.expr_as(
            Expr::col((Alias::new(ROOT_ALIAS), Alias::new(&output_name))),
            Alias::new(output_name),
        );
    }
    envelope.from_subquery(singleton.to_owned(), Alias::new(SINGLETON_ALIAS));
    envelope.join_lateral(
        JoinType::LeftJoin,
        root_query,
        Alias::new(ROOT_ALIAS),
        Expr::cust("true"),
    );
    envelope.to_owned()
}

/// Replaces numeric template markers against their ranges in the original
/// formatted SQL, so one replacement's payload is never scanned for another
/// marker. `None` asks the caller to rebuild with a fresh sentinel range.
fn replace_numeric_sentinels(sql: &str, replacements: &[(String, String)]) -> Option<String> {
    let mut ranges = Vec::with_capacity(replacements.len());
    for (needle, replacement) in replacements {
        let mut occurrences = sql.match_indices(needle);
        let (start, _) = occurrences.next()?;
        if occurrences.next().is_some() {
            return None;
        }
        ranges.push((start, start + needle.len(), replacement.as_str()));
    }
    ranges.sort_by_key(|(start, _, _)| *start);

    let mut rendered = String::with_capacity(sql.len());
    let mut cursor = 0;
    for (start, end, replacement) in ranges {
        // Distinct sentinels each occurring once identify disjoint render
        // sites. Treat any overlap as a collision and retry defensively.
        if start < cursor {
            return None;
        }
        rendered.push_str(&sql[cursor..start]);
        rendered.push_str(replacement);
        cursor = end;
    }
    rendered.push_str(&sql[cursor..]);
    Some(rendered)
}

fn generate_collection(
    collection: &CollectionPlan,
    catalog: &Catalog,
    output_name: &str,
    path: &[String],
    generation: SelectionGenerationContext<'_>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    match &collection.result {
        CollectionResultPlan::Rows(selection) => generate_rows(
            collection,
            selection,
            catalog,
            output_name,
            path,
            generation,
            template,
        ),
        CollectionResultPlan::Aggregate(aggregate) => generate_aggregate(
            collection,
            aggregate,
            catalog,
            output_name,
            path,
            generation,
            template,
        ),
    }
}

fn generate_rows(
    collection: &CollectionPlan,
    selection: &SelectionPlan,
    catalog: &Catalog,
    output_name: &str,
    path: &[String],
    generation: SelectionGenerationContext<'_>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    let current_table = table(catalog, collection.table)?;
    let context = context_for(current_table, output_name, path);
    let root_context = generation.root.unwrap_or(&context);
    let export_fields = generation.flattened;
    let encode_exported_fields = generation.parent.is_none();
    let mut query = Query::select();

    let relation_condition = if let Some((parent, foreign_key)) = generation.parent {
        Some(relation_condition(
            catalog,
            parent,
            &context,
            foreign_key,
            collection.table,
        )?)
    } else {
        None
    };
    let policy_filter = collection
        .policy_filter
        .as_ref()
        .map(|filter| {
            filter_expr(
                catalog, &context, &context, &context, None, filter, template,
            )
        })
        .transpose()?;
    let query_filter = collection
        .clauses
        .filter
        .as_ref()
        .map(|filter| {
            filter_expr(
                catalog,
                &context,
                root_context,
                &context,
                None,
                filter,
                template,
            )
        })
        .transpose()?;
    let filter = combine_filters(policy_filter, query_filter);
    if collection.shape.cardinality == SelectionCardinality::Collection
        && should_use_source_subquery(&collection.clauses, generation.options)
    {
        let source = limited_source_query(
            catalog,
            current_table,
            &context,
            relation_condition,
            filter,
            &collection.clauses,
            effective_limit(&collection.clauses, generation.options),
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
        apply_order_limit_offset(
            catalog,
            &context,
            &collection.clauses,
            None,
            &mut query,
            template,
        )?;
    };

    for (item_index, item) in selection.items.iter().enumerate() {
        let SelectionPlanItem::Relation(relation) = item else {
            continue;
        };
        let related_table = table(catalog, relation.collection.table)?;
        let relation_path =
            relation_instance_path(selection, item_index, related_table, relation, path);
        let foreign_key = foreign_key(catalog, relation.foreign_key)?;
        let child_context = context_for(related_table, &relation.output_name, &relation_path);
        let child_query = generate_collection(
            &relation.collection,
            catalog,
            &relation.output_name,
            &relation_path,
            SelectionGenerationContext {
                parent: Some((&context, foreign_key)),
                root: Some(root_context),
                options: generation.options,
                public_result_alias: None,
                flattened: relation.flattened,
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

    let fields = selection_field_expressions(selection, catalog, &context, path)?;
    if export_fields {
        for (output_name, expression) in fields {
            let expression = if encode_exported_fields {
                json_wire_expression(expression)
            } else {
                expression
            };
            query.expr_as(expression, Alias::new(output_name));
        }
        return Ok(query.to_owned());
    }
    let object = json_build_object(fields);
    let expression: Expr = match collection.shape.cardinality {
        SelectionCardinality::Collection => Func::coalesce([
            ordered_json_agg(object, &collection.clauses, catalog, &context)?,
            Expr::value("[]"),
        ])
        .into(),
        SelectionCardinality::AtMostOne => object,
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

fn ordered_json_agg(
    object: Expr,
    clauses: &SelectionClauses,
    catalog: &Catalog,
    context: &SelectionContext,
) -> Result<Expr, SqlGenerationError> {
    if clauses.order_by.is_empty() {
        return Ok(PgFunc::json_agg(object).into());
    }

    let mut expressions = vec![object];
    let mut order_items = Vec::with_capacity(clauses.order_by.len());
    for order in &clauses.order_by {
        let column = column(catalog, order.column)?;
        expressions.push(Expr::col((
            Alias::new(&context.table_alias),
            Alias::new(&column.name),
        )));
        let direction = match &order.direction {
            SortDirectionPlan::Asc | SortDirectionPlan::Variant { .. } => "ASC",
            SortDirectionPlan::Desc => "DESC",
        };
        order_items.push(format!("${} {direction}", expressions.len()));
    }
    Ok(Expr::cust_with_exprs(
        format!("JSON_AGG($1 ORDER BY {})", order_items.join(", ")),
        expressions,
    ))
}

fn generate_aggregate(
    collection: &CollectionPlan,
    aggregate: &AggregatePlan,
    catalog: &Catalog,
    output_name: &str,
    path: &[String],
    generation: SelectionGenerationContext<'_>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    let current_table = table(catalog, collection.table)?;
    let context = context_for(current_table, output_name, path);
    let root_context = generation.root.unwrap_or(&context);
    let export_fields = generation.flattened;
    let encode_exported_fields = generation.parent.is_none();
    let mut query = Query::select();
    query.from_as(
        (
            Alias::new(&current_table.schema),
            Alias::new(&current_table.name),
        ),
        Alias::new(&context.table_alias),
    );
    if let Some((parent, foreign_key)) = generation.parent {
        query.cond_where(relation_condition(
            catalog,
            parent,
            &context,
            foreign_key,
            collection.table,
        )?);
    }
    if let Some(filter) = &collection.policy_filter {
        query.and_where(filter_expr(
            catalog, &context, &context, &context, None, filter, template,
        )?);
    }
    if let Some(filter) = &collection.clauses.filter {
        query.and_where(filter_expr(
            catalog,
            &context,
            root_context,
            &context,
            None,
            filter,
            template,
        )?);
    }

    if aggregate.mode == AggregateMode::Grouped {
        return generate_grouped_aggregate(
            query,
            aggregate,
            catalog,
            &context,
            generation.public_result_alias,
        );
    }

    let mut fields = Vec::new();
    for field in &aggregate.fields {
        fields.push((
            field.output_name.clone(),
            public_scalar_expression(
                aggregate_expression(field, catalog, &context)?,
                field.data_type,
            ),
        ));
    }
    if export_fields {
        for (output_name, expression) in fields {
            let expression = if encode_exported_fields {
                json_wire_expression(expression)
            } else {
                expression
            };
            query.expr_as(expression, Alias::new(output_name));
        }
        return Ok(query.to_owned());
    }
    query.expr_as(
        json_build_object(fields),
        Alias::new(
            generation
                .public_result_alias
                .unwrap_or(&context.result_alias),
        ),
    );
    Ok(query.to_owned())
}

fn generate_grouped_aggregate(
    mut grouped: SelectStatement,
    aggregate: &AggregatePlan,
    catalog: &Catalog,
    context: &SelectionContext,
    public_result_alias: Option<&str>,
) -> Result<SelectStatement, SqlGenerationError> {
    for key in &aggregate.group_keys {
        let column = column(catalog, key.column)?;
        let grouped_column =
            || Expr::col((Alias::new(&context.table_alias), Alias::new(&column.name)));
        grouped.expr_as(
            public_scalar_expression(grouped_column(), key.data_type),
            Alias::new(&key.output_name),
        );
        grouped.add_group_by([grouped_column()]);
    }
    for field in &aggregate.fields {
        grouped.expr_as(
            public_scalar_expression(
                aggregate_expression(field, catalog, context)?,
                field.data_type,
            ),
            Alias::new(&field.output_name),
        );
    }

    let grouped_alias = generated_identifier(
        &context.table_alias,
        "_groups_",
        &short_hash(&context.table_alias),
    );
    let fields = aggregate
        .group_keys
        .iter()
        .map(|key| key.output_name.as_str())
        .chain(
            aggregate
                .fields
                .iter()
                .map(|field| field.output_name.as_str()),
        )
        .map(|output_name| {
            (
                output_name.to_string(),
                Expr::col((Alias::new(&grouped_alias), Alias::new(output_name))),
            )
        })
        .collect();
    let mut query = Query::select();
    query.from_subquery(grouped, Alias::new(grouped_alias));
    query.expr_as(
        Func::coalesce([
            PgFunc::json_agg(json_build_object(fields)).into(),
            Expr::value("[]"),
        ]),
        Alias::new(public_result_alias.unwrap_or(&context.result_alias)),
    );
    Ok(query.to_owned())
}

fn aggregate_expression(
    field: &crate::plan::AggregateProjection,
    catalog: &Catalog,
    context: &SelectionContext,
) -> Result<Expr, SqlGenerationError> {
    aggregate_value_expression(field.function, field.operand, catalog, context)
}

fn aggregate_value_expression(
    function: AggregateFunction,
    operand: Option<ColumnId>,
    catalog: &Catalog,
    context: &SelectionContext,
) -> Result<Expr, SqlGenerationError> {
    let operand = operand
        .map(|column_id| {
            let column = column(catalog, column_id)?;
            Ok(Expr::col((
                Alias::new(&context.table_alias),
                Alias::new(&column.name),
            )))
        })
        .transpose()?;
    Ok(match function {
        AggregateFunction::Count => {
            Func::count(operand.unwrap_or_else(|| Expr::col(Asterisk))).into()
        }
        AggregateFunction::Exists => Func::count(Expr::col(Asterisk)).gt(0),
        AggregateFunction::Min => {
            Func::min(operand.ok_or(SqlGenerationError::MissingAggregateOperand)?).into()
        }
        AggregateFunction::Max => {
            Func::max(operand.ok_or(SqlGenerationError::MissingAggregateOperand)?).into()
        }
        AggregateFunction::Sum => {
            Func::sum(operand.ok_or(SqlGenerationError::MissingAggregateOperand)?).into()
        }
        AggregateFunction::Avg => {
            Func::avg(operand.ok_or(SqlGenerationError::MissingAggregateOperand)?).into()
        }
    })
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
    if let Some(relation_condition) = relation_condition {
        query.cond_where(relation_condition);
    }
    if let Some(filter) = filter {
        query.and_where(filter);
    }
    apply_order_limit_offset(catalog, context, clauses, limit, &mut query, template)?;
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
    limit_override: Option<u64>,
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
    if let Some(limit) = limit_override.or_else(|| sql_value_u64(&clauses.limit)) {
        query.limit(limit);
    } else if let Some(SqlValue::Parameter(parameter)) = &clauses.limit {
        query.limit(template.numeric_parameter_sentinel(parameter)?);
    }
    if let Some(offset) = sql_value_u64(&clauses.offset) {
        query.offset(offset);
    } else if let Some(SqlValue::Parameter(parameter)) = &clauses.offset {
        query.offset(template.numeric_parameter_sentinel(parameter)?);
    }
    Ok(())
}

fn sql_value_u64(value: &Option<SqlValue>) -> Option<u64> {
    match value {
        Some(SqlValue::Literal(value)) => Some(*value),
        _ => None,
    }
}

fn combine_filters(left: Option<Expr>, right: Option<Expr>) -> Option<Expr> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.and(right)),
        (Some(filter), None) | (None, Some(filter)) => Some(filter),
        (None, None) => None,
    }
}

fn filter_expr(
    catalog: &Catalog,
    context: &SelectionContext,
    root: &SelectionContext,
    predicate_source: &SelectionContext,
    parent: Option<&SelectionContext>,
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
                FilterColumnScope::PredicateSource => predicate_source,
                FilterColumnScope::Parent => parent.unwrap_or(context),
            };
            Expr::col((Alias::new(&source.table_alias), Alias::new(&column.name)))
        }
        FilterExpr::Parameter(parameter) => Expr::cust(template.parameter(parameter)),
        FilterExpr::Literal(literal) => match literal {
            FilterLiteral::String(value) => Expr::value(value.clone()),
            FilterLiteral::Number(value) => match value.parse::<i64>() {
                Ok(value) => Expr::value(value),
                Err(_) => {
                    // The parser has already validated this token as a
                    // number. Preserve its source text: routing exact
                    // numerics through f64 would silently round them.
                    Expr::value(template.numeric_literal_sentinel(value)?)
                }
            },
            FilterLiteral::Bool(value) => Expr::value(*value),
            FilterLiteral::Null => Expr::cust("null"),
        },
        FilterExpr::Binary { left, op, right } => {
            if matches!(op, FilterOp::Eq | FilterOp::Ne)
                && let Some(operand) = null_comparison_operand(left, right)
            {
                let operand = filter_expr(
                    catalog,
                    context,
                    root,
                    predicate_source,
                    parent,
                    operand,
                    template,
                )?;
                return Ok(Expr::cust_with_expr(
                    if *op == FilterOp::Eq {
                        "$1 IS NULL"
                    } else {
                        "$1 IS NOT NULL"
                    },
                    operand,
                ));
            }
            if *op == FilterOp::Like {
                let left = filter_expr(
                    catalog,
                    context,
                    root,
                    predicate_source,
                    parent,
                    left,
                    template,
                )?;
                let right = filter_expr(
                    catalog,
                    context,
                    root,
                    predicate_source,
                    parent,
                    right,
                    template,
                )?;
                return Ok(Expr::cust_with_exprs("$1 like $2", [left, right]));
            }
            let left = filter_expr(
                catalog,
                context,
                root,
                predicate_source,
                parent,
                left,
                template,
            )?;
            let right = filter_expr(
                catalog,
                context,
                root,
                predicate_source,
                parent,
                right,
                template,
            )?;
            match op {
                FilterOp::Eq => left.eq(right),
                FilterOp::Ne => left.ne(right),
                FilterOp::Gt => left.gt(right),
                FilterOp::Ge => left.gte(right),
                FilterOp::Lt => left.lt(right),
                FilterOp::Le => left.lte(right),
                // Handled before generic binary lowering above; kept as an
                // error rather than a panic so drift cannot take down the
                // generator.
                FilterOp::Like => return Err(SqlGenerationError::UnsupportedFilterFragment),
                FilterOp::And => left.and(right),
                FilterOp::Or => left.or(right),
            }
        }
        FilterExpr::Not(operand) => Expr::cust_with_expr(
            "NOT ($1)",
            filter_expr(
                catalog,
                context,
                root,
                predicate_source,
                parent,
                operand,
                template,
            )?,
        ),
        FilterExpr::NullTest { operand, negated } => Expr::cust_with_expr(
            if *negated {
                "$1 IS NOT NULL"
            } else {
                "$1 IS NULL"
            },
            filter_expr(
                catalog,
                context,
                root,
                predicate_source,
                parent,
                operand,
                template,
            )?,
        ),
        FilterExpr::Membership {
            operand,
            collection,
            negated,
        } => {
            let operand = filter_expr_fragment(
                catalog,
                context,
                root,
                predicate_source,
                parent,
                operand,
                template,
            )?;
            match collection {
                FilterCollection::List(items) if items.is_empty() => {
                    Expr::cust(if *negated { "TRUE" } else { "FALSE" })
                }
                FilterCollection::List(items) => {
                    let items = items
                        .iter()
                        .map(|item| {
                            filter_expr_fragment(
                                catalog,
                                context,
                                root,
                                predicate_source,
                                parent,
                                item,
                                template,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Expr::cust(format!(
                        "{operand} {} ({})",
                        if *negated { "NOT IN" } else { "IN" },
                        items.join(", ")
                    ))
                }
                FilterCollection::Parameter(parameter) => Expr::cust(format!(
                    "{operand} {}({})",
                    if *negated { "<> ALL" } else { "= ANY" },
                    template.parameter(parameter)
                )),
            }
        }
        FilterExpr::VariantBinary {
            left,
            path,
            variants,
            right,
        } => {
            if let Some(operand) = null_comparison_operand(left, right) {
                Expr::cust(format!(
                    "{} {} null",
                    filter_expr_fragment(
                        catalog,
                        context,
                        root,
                        predicate_source,
                        parent,
                        operand,
                        template,
                    )?,
                    template.variant(path, variants),
                ))
            } else {
                Expr::cust(format!(
                    "{} {} {}",
                    filter_expr_fragment(
                        catalog,
                        context,
                        root,
                        predicate_source,
                        parent,
                        left,
                        template,
                    )?,
                    template.variant(path, variants),
                    filter_expr_fragment(
                        catalog,
                        context,
                        root,
                        predicate_source,
                        parent,
                        right,
                        template,
                    )?
                ))
            }
        }
        FilterExpr::Exists {
            foreign_key: foreign_key_id,
            table: table_id,
            kind,
            source_scope,
            policy_filter,
            filter,
        } => {
            let related_table = table(catalog, *table_id)?;
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
            let relation_source = match source_scope {
                FilterColumnScope::Current => context,
                FilterColumnScope::Root => root,
                FilterColumnScope::PredicateSource => predicate_source,
                FilterColumnScope::Parent => parent.unwrap_or(context),
            };
            if let Some(foreign_key_id) = foreign_key_id {
                query.cond_where(relation_condition(
                    catalog,
                    relation_source,
                    &exists_context,
                    foreign_key(catalog, *foreign_key_id)?,
                    *table_id,
                )?);
            }
            if let Some(policy_filter) = policy_filter {
                query.and_where(filter_expr(
                    catalog,
                    &exists_context,
                    &exists_context,
                    &exists_context,
                    Some(relation_source),
                    policy_filter,
                    template,
                )?);
            }
            if let Some(filter) = filter {
                let (filter_predicate_source, filter_parent) = match kind {
                    ExistsKind::Explicit => (&exists_context, Some(relation_source)),
                    ExistsKind::RelationshipPredicate => (predicate_source, parent),
                };
                query.and_where(filter_expr(
                    catalog,
                    &exists_context,
                    root,
                    filter_predicate_source,
                    filter_parent,
                    filter,
                    template,
                )?);
            }
            Expr::exists(query.to_owned())
        }
        FilterExpr::RelationAggregate {
            foreign_key: foreign_key_id,
            table: table_id,
            function,
            operand,
            policy_filter,
        } => {
            let related_table = table(catalog, *table_id)?;
            let foreign_key = foreign_key(catalog, *foreign_key_id)?;
            let aggregate_context = context_for(
                related_table,
                &related_table.name,
                &[table_label(related_table), function.label().to_string()],
            );
            let mut query = Query::select();
            query.from_as(
                (
                    Alias::new(&related_table.schema),
                    Alias::new(&related_table.name),
                ),
                Alias::new(&aggregate_context.table_alias),
            );
            query.cond_where(relation_condition(
                catalog,
                context,
                &aggregate_context,
                foreign_key,
                *table_id,
            )?);
            if let Some(policy_filter) = policy_filter {
                query.and_where(filter_expr(
                    catalog,
                    &aggregate_context,
                    &aggregate_context,
                    &aggregate_context,
                    Some(context),
                    policy_filter,
                    template,
                )?);
            }
            if *function == AggregateFunction::Exists {
                query.expr(Expr::value(1));
                Expr::exists(query.to_owned())
            } else {
                query.expr(aggregate_value_expression(
                    *function,
                    *operand,
                    catalog,
                    &aggregate_context,
                )?);
                Expr::SubQuery(None, Box::new(query.to_owned().into()))
            }
        }
    })
}

fn null_comparison_operand<'a>(
    left: &'a FilterExpr,
    right: &'a FilterExpr,
) -> Option<&'a FilterExpr> {
    if matches!(left, FilterExpr::Literal(FilterLiteral::Null)) {
        Some(right)
    } else if matches!(right, FilterExpr::Literal(FilterLiteral::Null)) {
        Some(left)
    } else {
        None
    }
}

fn filter_expr_fragment(
    catalog: &Catalog,
    context: &SelectionContext,
    root: &SelectionContext,
    predicate_source: &SelectionContext,
    parent: Option<&SelectionContext>,
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
                FilterColumnScope::PredicateSource => predicate_source,
                FilterColumnScope::Parent => parent.unwrap_or(context),
            };
            format!("\"{}\".\"{}\"", source.table_alias, column.name)
        }
        FilterExpr::Parameter(parameter) => template.parameter(parameter),
        FilterExpr::Literal(FilterLiteral::String(value)) => sql_string(value),
        FilterExpr::Literal(FilterLiteral::Number(value)) => value.clone(),
        FilterExpr::Literal(FilterLiteral::Bool(value)) => value.to_string(),
        FilterExpr::Literal(FilterLiteral::Null) => "null".to_string(),
        FilterExpr::Binary { .. }
        | FilterExpr::Not(_)
        | FilterExpr::NullTest { .. }
        | FilterExpr::Membership { .. }
        | FilterExpr::VariantBinary { .. }
        | FilterExpr::Exists { .. }
        | FilterExpr::RelationAggregate { .. } => {
            return Err(SqlGenerationError::UnsupportedFilterFragment);
        }
    })
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Extends a semantic result path with one nested selection instance.
/// Repeated flattened siblings can share a public relation name, so later
/// occurrences receive an internal-only discriminator.
fn relation_instance_path(
    selection: &SelectionPlan,
    item_index: usize,
    table: &Table,
    relation: &NestedRelation,
    parent_path: &[String],
) -> Vec<String> {
    let occurrence = selection.items[..item_index]
        .iter()
        .filter(|item| {
            matches!(
                item,
                SelectionPlanItem::Relation(candidate)
                    if candidate.collection.table == relation.collection.table
                        && candidate.output_name == relation.output_name
            )
        })
        .count();
    let mut segment = path_segment(table, &relation.output_name);
    if occurrence > 0 {
        segment.push('#');
        segment.push_str(&occurrence.to_string());
    }
    let mut path = parent_path.to_vec();
    path.push(segment);
    path
}

fn selection_field_expressions(
    selection: &SelectionPlan,
    catalog: &Catalog,
    context: &SelectionContext,
    path: &[String],
) -> Result<Vec<(String, Expr)>, SqlGenerationError> {
    let mut fields = Vec::new();
    for (item_index, item) in selection.items.iter().enumerate() {
        match item {
            SelectionPlanItem::Projection(projection) => {
                let column = column(catalog, projection.column)?;
                fields.push((
                    projection.output_name.clone(),
                    public_scalar_expression(
                        Expr::col((Alias::new(&context.table_alias), Alias::new(&column.name))),
                        column.data_type,
                    ),
                ));
            }
            SelectionPlanItem::Relation(relation) => {
                let table = table(catalog, relation.collection.table)?;
                let relation_path =
                    relation_instance_path(selection, item_index, table, relation, path);
                let related_context = context_for(table, &relation.output_name, &relation_path);
                if relation.flattened {
                    let mut output_names = Vec::new();
                    collect_collection_output_names(&relation.collection.result, &mut output_names);
                    fields.extend(output_names.into_iter().map(|output_name| {
                        (
                            output_name.clone(),
                            Expr::col((
                                Alias::new(&related_context.json_alias),
                                Alias::new(output_name),
                            )),
                        )
                    }));
                } else {
                    fields.push((
                        relation.output_name.clone(),
                        Expr::col((
                            Alias::new(&related_context.json_alias),
                            Alias::new(&related_context.result_alias),
                        )),
                    ));
                }
            }
        }
    }
    Ok(fields)
}

fn collect_collection_output_names(result: &CollectionResultPlan, names: &mut Vec<String>) {
    match result {
        CollectionResultPlan::Rows(selection) => {
            for item in &selection.items {
                match item {
                    SelectionPlanItem::Projection(projection) => {
                        names.push(projection.output_name.clone());
                    }
                    SelectionPlanItem::Relation(relation) if relation.flattened => {
                        collect_collection_output_names(&relation.collection.result, names);
                    }
                    SelectionPlanItem::Relation(relation) => {
                        names.push(relation.output_name.clone());
                    }
                }
            }
        }
        CollectionResultPlan::Aggregate(aggregate) => names.extend(
            aggregate
                .fields
                .iter()
                .map(|field| field.output_name.clone()),
        ),
    }
}

fn json_build_object(fields: Vec<(String, Expr)>) -> Expr {
    PgFunc::json_build_object(
        fields
            .into_iter()
            .map(|(output_name, expression)| (Expr::value(output_name), expression))
            .collect(),
    )
    .into()
}

/// Converts one scalar expression to its public JSON wire representation.
/// Exact numerics cross the JSON boundary as text so host runtimes cannot
/// silently round them through an IEEE-754 number.
fn public_scalar_expression(expression: Expr, data_type: DataType) -> Expr {
    if data_type == DataType::Numeric {
        expression.cast_as(Alias::new("text"))
    } else {
        expression
    }
}

/// Gives root-flattened result columns the same public JSON value encoding
/// they would receive inside [`json_build_object`].
fn json_wire_expression(expression: Expr) -> Expr {
    Expr::cust_with_expr("TO_JSON($1)", expression)
}

fn relation_condition(
    catalog: &Catalog,
    parent: &SelectionContext,
    child: &SelectionContext,
    foreign_key: &ForeignKey,
    child_table: TableId,
) -> Result<Condition, SqlGenerationError> {
    let mut condition = Condition::all();
    let (child_columns, parent_columns) = if child_table == foreign_key.from_table {
        (&foreign_key.from_columns, &foreign_key.to_columns)
    } else {
        (&foreign_key.to_columns, &foreign_key.from_columns)
    };
    for (child_column, parent_column) in child_columns.iter().zip(parent_columns) {
        condition = condition.add(
            Expr::col((
                Alias::new(&child.table_alias),
                Alias::new(&column(catalog, *child_column)?.name),
            ))
            .equals((
                Alias::new(&parent.table_alias),
                Alias::new(&column(catalog, *parent_column)?.name),
            )),
        );
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
    format!("{:08x}", hash as u32)
}

fn table(catalog: &Catalog, id: TableId) -> Result<&Table, SqlGenerationError> {
    catalog
        .table_by_id(id)
        .ok_or(SqlGenerationError::MissingTable(id.0))
}

fn column(catalog: &Catalog, id: ColumnId) -> Result<&Column, SqlGenerationError> {
    catalog
        .column_by_id(id)
        .ok_or(SqlGenerationError::MissingColumn(id.0))
}

fn foreign_key(catalog: &Catalog, id: ForeignKeyId) -> Result<&ForeignKey, SqlGenerationError> {
    catalog
        .foreign_key_by_id(id)
        .ok_or(SqlGenerationError::MissingForeignKey(id.0))
}

fn table_label(table: &Table) -> String {
    format!("{}.{}", table.schema, table.name)
}
