use crate::catalog::{
    Catalog, CatalogType, CatalogValueShape, Column, ColumnId, DataType, Relation, RelationId,
    Table, TableId, TypeKey, WireEncoding,
};
use crate::entities::aggregate::{AggregateFunction, AggregateMode};
use crate::plan::{
    AggregatePlan, CollectionPlan, CollectionResultPlan, DynamicInputFieldPlan, DynamicInputKind,
    DynamicOrderDirection, DynamicPredicateOperator, ExistsKind, FilterCollection,
    FilterColumnScope, FilterExpr, FilterLiteral, FilterOp, NestedRelation, OrderByPlan,
    PolicyContextRequirement, PolicyFieldFilter, PolicyFieldTarget, QueryPlan, QueryRootPlan,
    SelectionClauses, SelectionPlan, SelectionPlanItem, SortDirectionPlan, SqlParameter, SqlValue,
    SqlVariantCase,
};
use crate::resolution::SelectionCardinality;
use sea_query::{
    Alias, Asterisk, Condition, Expr, ExprTrait, Func, IntoIden, JoinType, Order, PgFunc,
    PostgresQueryBuilder, Query, SelectStatement, TableRef,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedSql {
    pub operation_name: String,
    /// Human-readable SQL formatted for diagnostics and development output.
    pub sql: String,
    /// Semantically identical single-line SQL for compact generated output.
    pub compact_sql: String,
    pub parameters: Vec<GeneratedSqlParameter>,
    /// Trusted policy context reached while rendering readable-view guards.
    pub policy_context: Vec<PolicyContextRequirement>,
    pub variants: Vec<GeneratedSqlVariant>,
    pub dynamic_sites: Vec<GeneratedDynamicInputSite>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedSqlParameter {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedSqlVariant {
    pub path: String,
    pub cases: Vec<SqlVariantCase>,
    pub null_text: Option<String>,
}

/// Compiler-owned SQL replacement data for one bounded dynamic usage site.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedDynamicInputSite {
    pub path: String,
    pub kind: DynamicInputKind,
    pub marker: String,
    pub identity_sql: String,
    pub fields: Vec<GeneratedDynamicInputSiteField>,
}

/// Site-specific readable SQL capabilities for one preset scalar.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedDynamicInputSiteField {
    pub key: String,
    pub data_type: DataType,
    pub operators: Vec<GeneratedDynamicPredicateOperator>,
    pub directions: Vec<SqlVariantCase>,
}

/// Structured SQL lowering for one public predicate operator at one site.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratedDynamicPredicateOperator {
    pub name: DynamicPredicateOperator,
    pub value_kind: GeneratedDynamicValueKind,
    pub before_value: Option<String>,
    pub after_value: Option<String>,
    pub cases: Vec<SqlVariantCase>,
}

/// Runtime operand shape consumed by a generated predicate lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeneratedDynamicValueKind {
    Scalar,
    Collection,
    Boolean,
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
    #[error("relation id `{0}` was not found in catalog")]
    MissingRelation(usize),
    #[error("aggregate function requires a planned operand")]
    MissingAggregateOperand,
    #[error("filter shape is not supported in a text fragment position")]
    UnsupportedFilterFragment,
    #[error("failed to render a dynamic input field expression")]
    DynamicExpressionRender,
    #[error("failed to locate the generated dynamic SQL marker `{0}`")]
    DynamicMarkerMissing(String),
    #[error("generated dynamic order marker `{0}` is not followed by `ASC`")]
    DynamicOrderMarkerRender(String),
    #[error("SQL template numeric sentinel space was exhausted")]
    TemplateSentinelExhausted,
    #[error("query plan has no roots")]
    EmptyQueryPlan,
}

#[derive(Clone, Debug)]
struct SelectionContext {
    table_alias: String,
    json_alias: String,
    result_alias: String,
}

#[derive(Clone, Copy)]
struct SqlRowView<'a> {
    context: &'a SelectionContext,
    field_filters: &'a [PolicyFieldFilter],
}

impl<'a> SqlRowView<'a> {
    fn readable(context: &'a SelectionContext, field_filters: &'a [PolicyFieldFilter]) -> Self {
        Self {
            context,
            field_filters,
        }
    }

    fn raw(context: &'a SelectionContext) -> Self {
        Self {
            context,
            field_filters: &[],
        }
    }
}

#[derive(Clone, Copy)]
struct FilterSqlScope<'a> {
    current: SqlRowView<'a>,
    root: SqlRowView<'a>,
    predicate_source: SqlRowView<'a>,
    parent: Option<SqlRowView<'a>>,
}

impl<'a> FilterSqlScope<'a> {
    fn raw(context: &'a SelectionContext) -> Self {
        let raw = SqlRowView::raw(context);
        Self {
            current: raw,
            root: raw,
            predicate_source: raw,
            parent: None,
        }
    }
}

struct SelectionGenerationContext<'a> {
    parent: Option<(SqlRowView<'a>, &'a Relation)>,
    root: Option<SqlRowView<'a>>,
    options: PostgresSqlOptions,
    public_result_alias: Option<&'a str>,
    flattened: bool,
}

struct SqlTemplateContext {
    parameters: Vec<GeneratedSqlParameter>,
    policy_context: Vec<PolicyContextRequirement>,
    variants: Vec<GeneratedSqlVariant>,
    replacements: Vec<(String, String)>,
    numeric_replacements: Vec<(String, String)>,
    dynamic_markers: Vec<DynamicMarker>,
    dynamic_sites: Vec<GeneratedDynamicInputSite>,
    next_sentinel: u64,
    next_dynamic_marker: u64,
}

const FIRST_NUMERIC_SENTINEL: u64 = 9_000_000_000_000_000_000;

#[derive(Clone, Copy)]
enum DynamicMarkerKind {
    Exact,
    Order,
}

struct DynamicMarker {
    internal: String,
    public: String,
    kind: DynamicMarkerKind,
}

impl SqlTemplateContext {
    fn new(next_sentinel: u64, next_dynamic_marker: u64) -> Self {
        Self {
            parameters: Vec::new(),
            policy_context: Vec::new(),
            variants: Vec::new(),
            replacements: Vec::new(),
            numeric_replacements: Vec::new(),
            dynamic_markers: Vec::new(),
            dynamic_sites: Vec::new(),
            next_sentinel,
            next_dynamic_marker,
        }
    }

    fn parameter(&mut self, parameter: &SqlParameter) -> String {
        let placeholder = if let Some(index) = self
            .parameters
            .iter()
            .position(|candidate| candidate.path == parameter.path)
        {
            format!("${}", index + 1)
        } else {
            self.parameters.push(GeneratedSqlParameter {
                path: parameter.path.clone(),
            });
            format!("${}", self.parameters.len())
        };
        parameter
            .text_cast
            .as_ref()
            .map_or(placeholder.clone(), |target| {
                text_cast_parameter(&placeholder, target, parameter.collection)
            })
    }

    fn require_policy_context(&mut self, requirements: &[PolicyContextRequirement]) {
        for requirement in requirements {
            if !self.policy_context.contains(requirement) {
                self.policy_context.push(requirement.clone());
            }
        }
    }

    fn variant(&mut self, path: &str, cases: &[SqlVariantCase]) -> String {
        self.variant_with_null(path, cases, None)
    }

    fn nullable_variant(&mut self, path: &str, cases: &[SqlVariantCase]) -> String {
        self.variant_with_null(path, cases, Some("null".to_string()))
    }

    fn variant_with_null(
        &mut self,
        path: &str,
        cases: &[SqlVariantCase],
        null_text: Option<String>,
    ) -> String {
        if !self.variants.iter().any(|variant| variant.path == path) {
            self.variants.push(GeneratedSqlVariant {
                path: path.to_string(),
                cases: cases.to_vec(),
                null_text,
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
        let replacement = format!("\"{table_alias}\".\"{column}\" {placeholder}");
        for direction in ["asc", "ASC"] {
            self.replacements.push((
                format!("\"{table_alias}\".\"{column}\" {direction}"),
                replacement.clone(),
            ));
        }
    }

    fn numeric_parameter_sentinel(
        &mut self,
        parameter: &SqlParameter,
    ) -> Result<u64, SqlGenerationError> {
        let placeholder = self.parameter(parameter);
        self.numeric_sentinel(placeholder)
    }

    fn guarded_numeric_parameter_sentinel(
        &mut self,
        parameter: &SqlParameter,
        guard: u64,
    ) -> Result<u64, SqlGenerationError> {
        let placeholder = self.parameter(parameter);
        self.numeric_sentinel(format!("LEAST(COALESCE({placeholder}, {guard}), {guard})"))
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

    fn dynamic_site(
        &mut self,
        path: &str,
        kind: DynamicInputKind,
        identity_sql: &str,
        fields: Vec<GeneratedDynamicInputSiteField>,
        marker_kind: DynamicMarkerKind,
    ) -> Result<String, SqlGenerationError> {
        let sentinel = self.next_sentinel;
        self.next_sentinel = sentinel
            .checked_add(1)
            .ok_or(SqlGenerationError::TemplateSentinelExhausted)?;
        let internal = format!("__dsql_dynamic_{sentinel}__");
        let public = format!("{{{{dynamic:{}}}}}", self.next_dynamic_marker);
        self.next_dynamic_marker = self
            .next_dynamic_marker
            .checked_add(1)
            .ok_or(SqlGenerationError::TemplateSentinelExhausted)?;
        self.dynamic_markers.push(DynamicMarker {
            internal: internal.clone(),
            public: public.clone(),
            kind: marker_kind,
        });
        self.dynamic_sites.push(GeneratedDynamicInputSite {
            path: path.to_string(),
            kind,
            marker: public,
            identity_sql: identity_sql.to_string(),
            fields,
        });
        Ok(internal)
    }
}

pub fn generate_postgres_sql(
    operation_name: &str,
    plan: &QueryPlan,
    catalog: &Catalog,
) -> Result<GeneratedSql, SqlGenerationError> {
    generate_postgres_sql_with_options(operation_name, plan, catalog, PostgresSqlOptions::default())
}

pub fn generate_postgres_sql_with_options(
    operation_name: &str,
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
    let mut next_dynamic_marker = 0;
    loop {
        let mut template = SqlTemplateContext::new(next_sentinel, next_dynamic_marker);
        let mut roots = Vec::with_capacity(plan.roots.len());
        for root in &plan.roots {
            roots.push(generate_root_query(root, catalog, options, &mut template)?);
        }
        let query = if roots.len() == 1 {
            let Some((query, _)) = roots.pop() else {
                return Err(SqlGenerationError::EmptyQueryPlan);
            };
            query
        } else {
            combine_root_queries(roots)?
        };
        let compact = query.to_string(PostgresQueryBuilder);
        let formatted = sqlformat::format(
            &compact,
            &sqlformat::QueryParams::default(),
            &format_options,
        );
        let Some(sql) = finalize_sql_template(&formatted, &template)? else {
            next_sentinel = template.next_sentinel;
            next_dynamic_marker = template.next_dynamic_marker;
            continue;
        };
        let Some(compact_sql) = finalize_sql_template(&compact, &template)? else {
            next_sentinel = template.next_sentinel;
            next_dynamic_marker = template.next_dynamic_marker;
            continue;
        };
        return Ok(GeneratedSql {
            operation_name: operation_name.to_string(),
            sql,
            compact_sql,
            parameters: template.parameters,
            policy_context: template.policy_context,
            variants: template.variants,
            dynamic_sites: template.dynamic_sites,
        });
    }
}

fn generate_root_query(
    plan: &QueryRootPlan,
    catalog: &Catalog,
    options: PostgresSqlOptions,
    template: &mut SqlTemplateContext,
) -> Result<(SelectStatement, Vec<String>), SqlGenerationError> {
    let mut path = Vec::new();
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
        template,
    )?;
    let output_names = root_output_names(plan);
    let root_query = if matches!(plan.collection.result, CollectionResultPlan::Rows(_))
        && plan.collection.shape.cardinality == SelectionCardinality::AtMostOne
    {
        singular_root_envelope(&output_names, root_query)
    } else {
        root_query
    };
    Ok((root_query, output_names))
}

fn combine_root_queries(
    roots: Vec<(SelectStatement, Vec<String>)>,
) -> Result<SelectStatement, SqlGenerationError> {
    if roots.is_empty() {
        return Err(SqlGenerationError::EmptyQueryPlan);
    }

    let mut query = Query::select();
    for (index, (root, output_names)) in roots.into_iter().enumerate() {
        let root_alias = format!("root_{index}");
        for output_name in output_names {
            query.expr_as(
                Expr::col((Alias::new(&root_alias), Alias::new(&output_name))),
                Alias::new(output_name),
            );
        }
        if index == 0 {
            query.from_subquery(root, Alias::new(root_alias));
        } else {
            query.cross_join(TableRef::SubQuery(
                root.into(),
                Alias::new(root_alias).into_iden(),
            ));
        }
    }
    Ok(query.to_owned())
}

fn root_output_names(plan: &QueryRootPlan) -> Vec<String> {
    let mut output_names = if plan.flattened {
        let mut names = Vec::new();
        collect_collection_output_names(&plan.collection.result, &mut names);
        names
    } else {
        vec![plan.output_name.clone()]
    };
    output_names.dedup();
    output_names
}

fn singular_root_envelope(output_names: &[String], root_query: SelectStatement) -> SelectStatement {
    const SINGLETON_ALIAS: &str = "singleton";
    const ROOT_ALIAS: &str = "root";

    let mut singleton = Query::select();
    singleton.expr(Expr::value(1));

    let mut envelope = Query::select();
    for output_name in output_names {
        envelope.expr_as(
            Expr::col((Alias::new(ROOT_ALIAS), Alias::new(output_name))),
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

fn finalize_sql_template(
    sql: &str,
    template: &SqlTemplateContext,
) -> Result<Option<String>, SqlGenerationError> {
    let Some(sql) = replace_dynamic_markers(sql, &template.dynamic_markers)? else {
        return Ok(None);
    };
    let Some(mut sql) = replace_numeric_sentinels(&sql, &template.numeric_replacements) else {
        return Ok(None);
    };
    for (needle, replacement) in &template.replacements {
        sql = sql.replace(needle, replacement);
    }
    Ok(Some(sql))
}

fn replace_dynamic_markers(
    sql: &str,
    markers: &[DynamicMarker],
) -> Result<Option<String>, SqlGenerationError> {
    let mut ranges = Vec::with_capacity(markers.len());
    for marker in markers {
        if sql.contains(&marker.public) {
            return Ok(None);
        }
        let mut occurrences = sql.match_indices(&marker.internal);
        let Some((start, _)) = occurrences.next() else {
            return Err(SqlGenerationError::DynamicMarkerMissing(
                marker.internal.clone(),
            ));
        };
        if occurrences.next().is_some() {
            return Ok(None);
        }
        let mut end = start + marker.internal.len();
        if matches!(marker.kind, DynamicMarkerKind::Order) {
            let rest = &sql[end..];
            let whitespace = rest.len() - rest.trim_start().len();
            end += whitespace;
            if sql[end..].starts_with("asc") || sql[end..].starts_with("ASC") {
                end += 3;
            } else {
                return Err(SqlGenerationError::DynamicOrderMarkerRender(
                    marker.internal.clone(),
                ));
            }
        }
        ranges.push((start, end, marker.public.as_str()));
    }
    Ok(replace_ranges(sql, ranges))
}

/// Replaces numeric template markers against their ranges in one rendered SQL
/// form, so one replacement's payload is never scanned for another marker.
/// `None` asks the caller to rebuild with a fresh sentinel range.
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
    replace_ranges(sql, ranges)
}

fn replace_ranges(sql: &str, mut ranges: Vec<(usize, usize, &str)>) -> Option<String> {
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
    let current_view = SqlRowView::readable(&context, &collection.field_filters);
    let root_view = generation.root.unwrap_or(current_view);
    let export_fields = generation.flattened;
    let encode_exported_fields = generation.parent.is_none();
    let mut query = Query::select();

    let relation_condition = if let Some((parent, relation)) = generation.parent {
        Some(relation_condition(
            catalog,
            parent.context,
            &context,
            relation,
        )?)
    } else {
        None
    };
    let relation_filter = generation
        .parent
        .and_then(|(parent, relation)| {
            policy_field_filter(
                parent.field_filters,
                PolicyFieldTarget::Relation(relation.id),
            )
            .map(|filter| render_policy_field_filter(catalog, parent.context, filter, template))
        })
        .transpose()?;
    let policy_filter = collection
        .policy_filter
        .as_ref()
        .map(|filter| filter_expr(catalog, FilterSqlScope::raw(&context), filter, template))
        .transpose()?;
    let query_filter = collection
        .clauses
        .filter
        .as_ref()
        .map(|filter| {
            filter_expr(
                catalog,
                FilterSqlScope {
                    current: current_view,
                    root: root_view,
                    predicate_source: current_view,
                    parent: generation.parent.map(|(parent, _)| parent),
                },
                filter,
                template,
            )
        })
        .transpose()?;
    let filter = combine_filters(
        combine_filters(relation_filter, policy_filter),
        query_filter,
    );
    if collection.shape.cardinality == SelectionCardinality::Collection
        && should_use_source_subquery(&collection.clauses, generation.options)
    {
        let source = limited_source_query(
            catalog,
            current_table,
            current_view,
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
            current_view,
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
        let relation_fact = catalog_relation(catalog, relation.relation)?;
        let child_context = context_for(related_table, &relation.output_name, &relation_path);
        let child_query = generate_collection(
            &relation.collection,
            catalog,
            &relation.output_name,
            &relation_path,
            SelectionGenerationContext {
                parent: Some((current_view, relation_fact)),
                root: Some(root_view),
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

    let fields = selection_field_expressions(selection, catalog, current_view, path, template)?;
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
            ordered_json_agg(object, &collection.clauses, catalog, current_view, template)?,
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
    context: SqlRowView<'_>,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    if clauses.order_by.is_empty() {
        return Ok(PgFunc::json_agg(object).into());
    }

    let mut expressions = vec![object];
    let mut order_items = Vec::with_capacity(clauses.order_by.len());
    for order in &clauses.order_by {
        match order {
            OrderByPlan::Column { column, direction } => {
                let expression = masked_column_expression(catalog, context, *column, template)?;
                match direction {
                    SortDirectionPlan::Asc => {
                        expressions.push(expression);
                        order_items.push(format!("${} ASC", expressions.len()));
                    }
                    SortDirectionPlan::Desc => {
                        expressions.push(expression);
                        order_items.push(format!("${} DESC", expressions.len()));
                    }
                    SortDirectionPlan::Variant {
                        path,
                        variants,
                        nullable,
                    } => {
                        let placeholder = if *nullable {
                            template.nullable_variant(path, variants)
                        } else {
                            template.variant(path, variants)
                        };
                        expressions.push(expression.clone());
                        order_items.push(format!(
                            "CASE WHEN '{placeholder}' = 'asc' THEN ${} END ASC",
                            expressions.len()
                        ));
                        expressions.push(expression);
                        order_items.push(format!(
                            "CASE WHEN '{placeholder}' = 'desc' THEN ${} END DESC",
                            expressions.len()
                        ));
                    }
                }
            }
            OrderByPlan::Dynamic { path, fields, .. } => {
                let fields = generated_dynamic_order_fields(catalog, context, fields, template)?;
                let marker = template.dynamic_site(
                    path,
                    DynamicInputKind::Order,
                    "NULL::integer",
                    fields,
                    DynamicMarkerKind::Exact,
                )?;
                order_items.push(marker);
            }
        }
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
    let current_view = SqlRowView::readable(&context, &collection.field_filters);
    let root_view = generation.root.unwrap_or(current_view);
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
    if let Some((parent, relation)) = generation.parent {
        query.cond_where(relation_condition(
            catalog,
            parent.context,
            &context,
            relation,
        )?);
        if let Some(filter) = policy_field_filter(
            parent.field_filters,
            PolicyFieldTarget::Relation(relation.id),
        ) {
            query.and_where(render_policy_field_filter(
                catalog,
                parent.context,
                filter,
                template,
            )?);
        }
    }
    if let Some(filter) = &collection.policy_filter {
        query.and_where(filter_expr(
            catalog,
            FilterSqlScope::raw(&context),
            filter,
            template,
        )?);
    }
    if let Some(filter) = &collection.clauses.filter {
        query.and_where(filter_expr(
            catalog,
            FilterSqlScope {
                current: current_view,
                root: root_view,
                predicate_source: current_view,
                parent: generation.parent.map(|(parent, _)| parent),
            },
            filter,
            template,
        )?);
    }

    if aggregate.mode == AggregateMode::Grouped {
        return generate_grouped_aggregate(
            query,
            aggregate,
            catalog,
            current_view,
            generation.public_result_alias,
            template,
        );
    }

    let mut fields = Vec::new();
    for field in &aggregate.fields {
        fields.push((
            field.output_name.clone(),
            public_aggregate_expression(
                aggregate_expression(field, catalog, current_view, template)?,
                catalog,
                field,
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
    context: SqlRowView<'_>,
    public_result_alias: Option<&str>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    for key in &aggregate.group_keys {
        let grouped_column = masked_column_expression(catalog, context, key.column, template)?;
        grouped.expr_as(
            public_column_expression(grouped_column.clone(), catalog, key.column),
            Alias::new(&key.output_name),
        );
        grouped.add_group_by([grouped_column]);
    }
    for field in &aggregate.fields {
        grouped.expr_as(
            public_aggregate_expression(
                aggregate_expression(field, catalog, context, template)?,
                catalog,
                field,
            ),
            Alias::new(&field.output_name),
        );
    }

    let grouped_alias = generated_identifier(
        &context.context.table_alias,
        "_groups_",
        &short_hash(&context.context.table_alias),
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
        Alias::new(public_result_alias.unwrap_or(&context.context.result_alias)),
    );
    Ok(query.to_owned())
}

fn aggregate_expression(
    field: &crate::plan::AggregateProjection,
    catalog: &Catalog,
    context: SqlRowView<'_>,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    aggregate_value_expression(field.function, field.operand, catalog, context, template)
}

fn aggregate_value_expression(
    function: AggregateFunction,
    operand: Option<ColumnId>,
    catalog: &Catalog,
    context: SqlRowView<'_>,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    let operand = operand
        .map(|column_id| masked_column_expression(catalog, context, column_id, template))
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
    context: SqlRowView<'_>,
    relation_condition: Option<Condition>,
    filter: Option<Expr>,
    clauses: &SelectionClauses,
    limit: Option<u64>,
    template: &mut SqlTemplateContext,
) -> Result<SelectStatement, SqlGenerationError> {
    let mut query = Query::select();
    query.column(Asterisk).from_as(
        (Alias::new(&table.schema), Alias::new(&table.name)),
        Alias::new(&context.context.table_alias),
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
    context: SqlRowView<'_>,
    clauses: &SelectionClauses,
    limit_override: Option<u64>,
    query: &mut SelectStatement,
    template: &mut SqlTemplateContext,
) -> Result<(), SqlGenerationError> {
    for order in &clauses.order_by {
        match order {
            OrderByPlan::Column {
                column: column_id,
                direction,
            } => match direction {
                SortDirectionPlan::Asc => {
                    let expression =
                        masked_column_expression(catalog, context, *column_id, template)?;
                    query.order_by_expr(expression, Order::Asc);
                }
                SortDirectionPlan::Desc => {
                    let expression =
                        masked_column_expression(catalog, context, *column_id, template)?;
                    query.order_by_expr(expression, Order::Desc);
                }
                SortDirectionPlan::Variant {
                    path,
                    variants,
                    nullable,
                } => {
                    if !nullable
                        && policy_field_filter(
                            context.field_filters,
                            PolicyFieldTarget::Column(*column_id),
                        )
                        .is_none()
                    {
                        let column = column(catalog, *column_id)?;
                        query.order_by(
                            (
                                Alias::new(&context.context.table_alias),
                                Alias::new(&column.name),
                            ),
                            Order::Asc,
                        );
                        template.replace_order_direction(
                            &context.context.table_alias,
                            &column.name,
                            path,
                            variants,
                        );
                        continue;
                    }
                    let expression =
                        masked_column_expression(catalog, context, *column_id, template)?;
                    let placeholder = if *nullable {
                        template.nullable_variant(path, variants)
                    } else {
                        template.variant(path, variants)
                    };
                    query.order_by_expr(
                        Expr::cust_with_expr(
                            format!("CASE WHEN '{placeholder}' = 'asc' THEN $1 END"),
                            expression.clone(),
                        ),
                        Order::Asc,
                    );
                    query.order_by_expr(
                        Expr::cust_with_expr(
                            format!("CASE WHEN '{placeholder}' = 'desc' THEN $1 END"),
                            expression,
                        ),
                        Order::Desc,
                    );
                }
            },
            OrderByPlan::Dynamic { path, fields, .. } => {
                let fields = generated_dynamic_order_fields(catalog, context, fields, template)?;
                let marker = template.dynamic_site(
                    path,
                    DynamicInputKind::Order,
                    "NULL::integer",
                    fields,
                    DynamicMarkerKind::Order,
                )?;
                query.order_by_expr(Expr::cust(marker), Order::Asc);
            }
        }
    }
    match (&clauses.limit, limit_override) {
        (Some(SqlValue::Parameter(parameter)), Some(guard)) => {
            query.limit(template.guarded_numeric_parameter_sentinel(parameter, guard)?);
        }
        (Some(SqlValue::Parameter(parameter)), None) => {
            query.limit(template.numeric_parameter_sentinel(parameter)?);
        }
        (_, Some(limit)) => {
            query.limit(limit);
        }
        (Some(SqlValue::Literal(limit)), None) => {
            query.limit(*limit);
        }
        (None, None) => {}
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

fn policy_field_filter(
    filters: &[PolicyFieldFilter],
    target: PolicyFieldTarget,
) -> Option<&PolicyFieldFilter> {
    filters.iter().find(|filter| filter.target == target)
}

fn render_policy_field_filter(
    catalog: &Catalog,
    source: &SelectionContext,
    filter: &PolicyFieldFilter,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    template.require_policy_context(&filter.context);
    filter_expr(
        catalog,
        FilterSqlScope::raw(source),
        &filter.filter,
        template,
    )
}

fn masked_column_expression(
    catalog: &Catalog,
    source: SqlRowView<'_>,
    column_id: ColumnId,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    let column = column(catalog, column_id)?;
    let raw = Expr::col((
        Alias::new(&source.context.table_alias),
        Alias::new(&column.name),
    ));
    let Some(filter) =
        policy_field_filter(source.field_filters, PolicyFieldTarget::Column(column_id))
    else {
        return Ok(raw);
    };
    let guard = render_policy_field_filter(catalog, source.context, filter, template)?;
    Ok(Expr::case(guard, raw).finally(Expr::null()).into())
}

fn filter_column_source<'a>(
    scope: FilterSqlScope<'a>,
    column_scope: FilterColumnScope,
) -> SqlRowView<'a> {
    match column_scope {
        FilterColumnScope::Current => scope.current,
        FilterColumnScope::Root => scope.root,
        FilterColumnScope::PredicateSource => scope.predicate_source,
        FilterColumnScope::Parent => scope.parent.unwrap_or(scope.current),
    }
}

fn null_test_expr(
    catalog: &Catalog,
    scope: FilterSqlScope<'_>,
    operand: &FilterExpr,
    negated: bool,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    let (operand, guard) = if let FilterExpr::Column {
        scope: column_scope,
        column: column_id,
    } = operand
    {
        let source = filter_column_source(scope, *column_scope);
        let column = column(catalog, *column_id)?;
        let operand = Expr::col((
            Alias::new(&source.context.table_alias),
            Alias::new(&column.name),
        ));
        let guard =
            policy_field_filter(source.field_filters, PolicyFieldTarget::Column(*column_id))
                .map(|filter| render_policy_field_filter(catalog, source.context, filter, template))
                .transpose()?;
        (operand, guard)
    } else {
        (filter_expr(catalog, scope, operand, template)?, None)
    };
    let test = Expr::cust_with_expr(
        if negated {
            "$1 IS NOT NULL"
        } else {
            "$1 IS NULL"
        },
        operand,
    );
    Ok(if let Some(guard) = guard {
        guard.and(test)
    } else {
        test
    })
}

struct ConditionalFilter {
    present: Option<Expr>,
    value: Expr,
}

fn filter_expr(
    catalog: &Catalog,
    scope: FilterSqlScope<'_>,
    filter: &FilterExpr,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    let rendered = conditional_filter_expr(catalog, scope, filter, template)?;
    Ok(rendered.present.map_or(rendered.value.clone(), |present| {
        value_or_absent(rendered.value, present)
    }))
}

fn conditional_filter_expr(
    catalog: &Catalog,
    scope: FilterSqlScope<'_>,
    filter: &FilterExpr,
    template: &mut SqlTemplateContext,
) -> Result<ConditionalFilter, SqlGenerationError> {
    match filter {
        FilterExpr::Absent => Ok(ConditionalFilter {
            present: Some(Expr::cust("FALSE")),
            value: Expr::cust("TRUE"),
        }),
        FilterExpr::Optional { parameter, operand } => {
            let inner = conditional_filter_expr(catalog, scope, operand, template)?;
            let guard = Expr::cust(format!("{} IS NOT NULL", template.parameter(parameter)));
            let present = match inner.present {
                Some(present) => guard.and(present),
                None => guard,
            };
            Ok(ConditionalFilter {
                present: Some(present),
                value: inner.value,
            })
        }
        FilterExpr::Binary { left, op, right } if matches!(op, FilterOp::And | FilterOp::Or) => {
            let left = conditional_filter_expr(catalog, scope, left, template)?;
            let right = conditional_filter_expr(catalog, scope, right, template)?;
            Ok(combine_conditional(left, *op == FilterOp::And, right))
        }
        FilterExpr::Not(operand) => {
            let operand = conditional_filter_expr(catalog, scope, operand, template)?;
            Ok(ConditionalFilter {
                present: operand.present,
                value: not_expr(operand.value),
            })
        }
        _ => Ok(ConditionalFilter {
            present: None,
            value: filter_value_expr(catalog, scope, filter, template)?,
        }),
    }
}

fn combine_conditional(
    left: ConditionalFilter,
    and: bool,
    right: ConditionalFilter,
) -> ConditionalFilter {
    if and {
        match (left.present, right.present) {
            (None, None) => ConditionalFilter {
                present: None,
                value: left.value.and(right.value),
            },
            (None, Some(right_present)) => ConditionalFilter {
                present: None,
                value: left.value.and(value_or_absent(right.value, right_present)),
            },
            (Some(left_present), None) => ConditionalFilter {
                present: None,
                value: value_or_absent(left.value, left_present).and(right.value),
            },
            (Some(left_present), Some(right_present)) => {
                let present = left_present.clone().or(right_present.clone());
                ConditionalFilter {
                    present: Some(present),
                    value: value_or_absent(left.value, left_present)
                        .and(value_or_absent(right.value, right_present)),
                }
            }
        }
    } else {
        match (left.present, right.present) {
            (None, None) => ConditionalFilter {
                present: None,
                value: left.value.or(right.value),
            },
            (None, Some(right_present)) => ConditionalFilter {
                present: None,
                value: left.value.or(value_if_present(right.value, right_present)),
            },
            (Some(left_present), None) => ConditionalFilter {
                present: None,
                value: value_if_present(left.value, left_present).or(right.value),
            },
            (Some(left_present), Some(right_present)) => {
                let present = left_present.clone().or(right_present.clone());
                ConditionalFilter {
                    present: Some(present),
                    value: value_if_present(left.value, left_present)
                        .or(value_if_present(right.value, right_present)),
                }
            }
        }
    }
}

/// Renders the type-constraining value before its untyped presence guard.
///
/// PostgreSQL determines an external parameter's type from its first
/// constraining occurrence, so `$1 IS NOT NULL OR column LIKE $1` cannot be
/// prepared while `column LIKE $1 OR $1 IS NOT NULL` can.
fn value_or_absent(value: Expr, present: Expr) -> Expr {
    value.or(not_expr(present))
}

fn value_if_present(value: Expr, present: Expr) -> Expr {
    value.and(present)
}

fn not_expr(expression: Expr) -> Expr {
    Expr::cust_with_expr("NOT ($1)", expression)
}

fn filter_value_expr(
    catalog: &Catalog,
    scope: FilterSqlScope<'_>,
    filter: &FilterExpr,
    template: &mut SqlTemplateContext,
) -> Result<Expr, SqlGenerationError> {
    Ok(match filter {
        FilterExpr::Absent => {
            return Err(SqlGenerationError::UnsupportedFilterFragment);
        }
        FilterExpr::Optional { .. } => {
            return Err(SqlGenerationError::UnsupportedFilterFragment);
        }
        FilterExpr::Column {
            scope: column_scope,
            column: column_id,
        } => {
            let source = filter_column_source(scope, *column_scope);
            masked_column_expression(catalog, source, *column_id, template)?
        }
        FilterExpr::Parameter(parameter) => Expr::cust(template.parameter(parameter)),
        FilterExpr::DynamicPredicate { path, fields, .. } => {
            let fields =
                generated_dynamic_predicate_fields(catalog, scope.current, fields, template)?;
            Expr::cust(template.dynamic_site(
                path,
                DynamicInputKind::Predicate,
                "TRUE",
                fields,
                DynamicMarkerKind::Exact,
            )?)
        }
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
                return null_test_expr(catalog, scope, operand, *op == FilterOp::Ne, template);
            }
            if *op == FilterOp::Like {
                let left = filter_expr(catalog, scope, left, template)?;
                let right = filter_expr(catalog, scope, right, template)?;
                return Ok(Expr::cust_with_exprs("$1 like $2", [left, right]));
            }
            let left = filter_expr(catalog, scope, left, template)?;
            let right = filter_expr(catalog, scope, right, template)?;
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
        FilterExpr::Not(operand) => {
            Expr::cust_with_expr("NOT ($1)", filter_expr(catalog, scope, operand, template)?)
        }
        FilterExpr::NullTest { operand, negated } => {
            null_test_expr(catalog, scope, operand, *negated, template)?
        }
        FilterExpr::Membership {
            operand,
            collection,
            negated,
        } => {
            let operand = filter_expr(catalog, scope, operand, template)?;
            match collection {
                FilterCollection::List(items) if items.is_empty() => {
                    Expr::cust(if *negated { "TRUE" } else { "FALSE" })
                }
                FilterCollection::List(items) => {
                    let mut expressions = vec![operand];
                    expressions.extend(
                        items
                            .iter()
                            .map(|item| filter_expr(catalog, scope, item, template))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                    let placeholders = (2..=expressions.len())
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Expr::cust_with_exprs(
                        format!(
                            "$1 {} ({placeholders})",
                            if *negated { "NOT IN" } else { "IN" }
                        ),
                        expressions,
                    )
                }
                FilterCollection::Parameter(parameter) => Expr::cust_with_expr(
                    format!(
                        "$1 {}({})",
                        if *negated { "<> ALL" } else { "= ANY" },
                        template.parameter(parameter).replace('$', "$$")
                    ),
                    operand,
                ),
            }
        }
        FilterExpr::VariantBinary {
            left,
            path,
            variants,
            right,
        } => {
            if let Some(operand) = null_comparison_operand(left, right) {
                let operator = template.variant(path, variants);
                Expr::cust_with_expr(
                    format!("$1 {operator} null"),
                    filter_expr(catalog, scope, operand, template)?,
                )
            } else {
                let operator = template.variant(path, variants);
                Expr::cust_with_exprs(
                    format!("$1 {operator} $2"),
                    [
                        filter_expr(catalog, scope, left, template)?,
                        filter_expr(catalog, scope, right, template)?,
                    ],
                )
            }
        }
        FilterExpr::Exists {
            relation: relation_id,
            table: table_id,
            kind,
            source_scope,
            policy_filter,
            field_filters,
            filter,
        } => {
            let related_table = table(catalog, *table_id)?;
            let exists_context = context_for(
                related_table,
                &related_table.name,
                &[table_label(related_table)],
            );
            let exists_view = SqlRowView::readable(&exists_context, field_filters);
            let mut query = Query::select();
            query.expr(Expr::value(1)).from_as(
                (
                    Alias::new(&related_table.schema),
                    Alias::new(&related_table.name),
                ),
                Alias::new(&exists_context.table_alias),
            );
            let relation_source = match source_scope {
                FilterColumnScope::Current => scope.current,
                FilterColumnScope::Root => scope.root,
                FilterColumnScope::PredicateSource => scope.predicate_source,
                FilterColumnScope::Parent => scope.parent.unwrap_or(scope.current),
            };
            if let Some(relation_id) = relation_id {
                let relation = catalog_relation(catalog, *relation_id)?;
                query.cond_where(relation_condition(
                    catalog,
                    relation_source.context,
                    &exists_context,
                    relation,
                )?);
                if let Some(filter) = policy_field_filter(
                    relation_source.field_filters,
                    PolicyFieldTarget::Relation(relation.id),
                ) {
                    query.and_where(render_policy_field_filter(
                        catalog,
                        relation_source.context,
                        filter,
                        template,
                    )?);
                }
            }
            if let Some(policy_filter) = policy_filter {
                query.and_where(filter_expr(
                    catalog,
                    FilterSqlScope::raw(&exists_context),
                    policy_filter,
                    template,
                )?);
            }
            if let Some(filter) = filter {
                let (filter_predicate_source, filter_parent) = match kind {
                    ExistsKind::Explicit => (exists_view, Some(relation_source)),
                    ExistsKind::RelationshipPredicate => (scope.predicate_source, scope.parent),
                };
                query.and_where(filter_expr(
                    catalog,
                    FilterSqlScope {
                        current: exists_view,
                        root: scope.root,
                        predicate_source: filter_predicate_source,
                        parent: filter_parent,
                    },
                    filter,
                    template,
                )?);
            }
            Expr::exists(query.to_owned())
        }
        FilterExpr::RelationAggregate {
            relation: relation_id,
            table: table_id,
            function,
            operand,
            policy_filter,
            field_filters,
        } => {
            let related_table = table(catalog, *table_id)?;
            let relation = catalog_relation(catalog, *relation_id)?;
            let aggregate_context = context_for(
                related_table,
                &related_table.name,
                &[table_label(related_table), function.label().to_string()],
            );
            let aggregate_view = SqlRowView::readable(&aggregate_context, field_filters);
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
                scope.current.context,
                &aggregate_context,
                relation,
            )?);
            if let Some(filter) = policy_field_filter(
                scope.current.field_filters,
                PolicyFieldTarget::Relation(relation.id),
            ) {
                query.and_where(render_policy_field_filter(
                    catalog,
                    scope.current.context,
                    filter,
                    template,
                )?);
            }
            if let Some(policy_filter) = policy_filter {
                query.and_where(filter_expr(
                    catalog,
                    FilterSqlScope::raw(&aggregate_context),
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
                    aggregate_view,
                    template,
                )?);
                Expr::SubQuery(None, Box::new(query.to_owned().into()))
            }
        }
    })
}

fn generated_dynamic_predicate_fields(
    catalog: &Catalog,
    source: SqlRowView<'_>,
    fields: &[DynamicInputFieldPlan],
    template: &mut SqlTemplateContext,
) -> Result<Vec<GeneratedDynamicInputSiteField>, SqlGenerationError> {
    fields
        .iter()
        .map(|field| {
            let expression = dynamic_field_expression(catalog, source, field.column, template)?;
            let text_cast = catalog.type_for_column(field.column).and_then(|data_type| {
                (data_type.capabilities.wire == WireEncoding::TextCast).then_some(&data_type.key)
            });
            let operators = field
                .operators
                .iter()
                .map(|operator| {
                    let (value_kind, before_value, after_value, cases) = match operator {
                        DynamicPredicateOperator::Eq => dynamic_scalar(&expression, "=", text_cast),
                        DynamicPredicateOperator::Neq => {
                            dynamic_scalar(&expression, "!=", text_cast)
                        }
                        DynamicPredicateOperator::Gt => dynamic_scalar(&expression, ">", text_cast),
                        DynamicPredicateOperator::Gte => {
                            dynamic_scalar(&expression, ">=", text_cast)
                        }
                        DynamicPredicateOperator::Lt => dynamic_scalar(&expression, "<", text_cast),
                        DynamicPredicateOperator::Lte => {
                            dynamic_scalar(&expression, "<=", text_cast)
                        }
                        DynamicPredicateOperator::Like => {
                            dynamic_scalar(&expression, "like", text_cast)
                        }
                        DynamicPredicateOperator::In => (
                            GeneratedDynamicValueKind::Collection,
                            Some(format!(
                                "({expression}) = ANY({}",
                                dynamic_cast_before(text_cast)
                            )),
                            Some(format!("{})", dynamic_cast_after(text_cast, true))),
                            Vec::new(),
                        ),
                        DynamicPredicateOperator::NotIn => (
                            GeneratedDynamicValueKind::Collection,
                            Some(format!(
                                "({expression}) <> ALL({}",
                                dynamic_cast_before(text_cast)
                            )),
                            Some(format!("{})", dynamic_cast_after(text_cast, true))),
                            Vec::new(),
                        ),
                        DynamicPredicateOperator::IsNull => (
                            GeneratedDynamicValueKind::Boolean,
                            None,
                            None,
                            vec![
                                SqlVariantCase {
                                    value: "true".to_string(),
                                    text: format!("({expression}) IS NULL"),
                                },
                                SqlVariantCase {
                                    value: "false".to_string(),
                                    text: format!("({expression}) IS NOT NULL"),
                                },
                            ],
                        ),
                    };
                    GeneratedDynamicPredicateOperator {
                        name: *operator,
                        value_kind,
                        before_value,
                        after_value,
                        cases,
                    }
                })
                .collect();
            Ok(GeneratedDynamicInputSiteField {
                key: field.key.clone(),
                data_type: field.data_type,
                operators,
                directions: Vec::new(),
            })
        })
        .collect()
}

fn dynamic_scalar(
    expression: &str,
    operator: &str,
    text_cast: Option<&TypeKey>,
) -> (
    GeneratedDynamicValueKind,
    Option<String>,
    Option<String>,
    Vec<SqlVariantCase>,
) {
    (
        GeneratedDynamicValueKind::Scalar,
        Some(format!(
            "({expression}) {operator} {}",
            dynamic_cast_before(text_cast)
        )),
        text_cast.map(|_| dynamic_cast_after(text_cast, false)),
        Vec::new(),
    )
}

fn generated_dynamic_order_fields(
    catalog: &Catalog,
    source: SqlRowView<'_>,
    fields: &[DynamicInputFieldPlan],
    template: &mut SqlTemplateContext,
) -> Result<Vec<GeneratedDynamicInputSiteField>, SqlGenerationError> {
    fields
        .iter()
        .map(|field| {
            let expression = dynamic_field_expression(catalog, source, field.column, template)?;
            let directions = field
                .directions
                .iter()
                .map(|direction| SqlVariantCase {
                    value: direction.as_str().to_string(),
                    text: format!("({expression}) {}", dynamic_order_sql(*direction)),
                })
                .collect();
            Ok(GeneratedDynamicInputSiteField {
                key: field.key.clone(),
                data_type: field.data_type,
                operators: Vec::new(),
                directions,
            })
        })
        .collect()
}

fn dynamic_order_sql(direction: DynamicOrderDirection) -> &'static str {
    match direction {
        DynamicOrderDirection::Asc => "ASC",
        DynamicOrderDirection::Desc => "DESC",
        DynamicOrderDirection::AscNullsFirst => "ASC NULLS FIRST",
        DynamicOrderDirection::AscNullsLast => "ASC NULLS LAST",
        DynamicOrderDirection::DescNullsFirst => "DESC NULLS FIRST",
        DynamicOrderDirection::DescNullsLast => "DESC NULLS LAST",
    }
}

fn dynamic_field_expression(
    catalog: &Catalog,
    source: SqlRowView<'_>,
    column: ColumnId,
    template: &mut SqlTemplateContext,
) -> Result<String, SqlGenerationError> {
    let expression = masked_column_expression(catalog, source, column, template)?;
    let mut query = Query::select();
    query.expr(expression);
    let rendered = query.to_string(PostgresQueryBuilder);
    rendered
        .strip_prefix("SELECT ")
        .map(str::to_string)
        .ok_or(SqlGenerationError::DynamicExpressionRender)
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
    context: SqlRowView<'_>,
    path: &[String],
    template: &mut SqlTemplateContext,
) -> Result<Vec<(String, Expr)>, SqlGenerationError> {
    let mut fields = Vec::new();
    for (item_index, item) in selection.items.iter().enumerate() {
        match item {
            SelectionPlanItem::Projection(projection) => {
                let column = column(catalog, projection.column)?;
                fields.push((
                    projection.output_name.clone(),
                    public_column_expression(
                        masked_column_expression(catalog, context, projection.column, template)?,
                        catalog,
                        column.id,
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
/// Exact and provider-defined scalars cross the JSON boundary as text so host
/// runtimes never reinterpret their representation.
fn public_scalar_expression(expression: Expr, wire: WireEncoding) -> Expr {
    if matches!(
        wire,
        WireEncoding::BigInteger | WireEncoding::Numeric | WireEncoding::TextCast
    ) {
        expression.cast_as(Alias::new("text"))
    } else {
        expression
    }
}

fn public_column_expression(expression: Expr, catalog: &Catalog, column: ColumnId) -> Expr {
    if let Some(data_type) = catalog.type_for_column(column) {
        public_catalog_expression(expression, catalog, data_type)
    } else {
        expression
    }
}

fn public_catalog_expression(expression: Expr, catalog: &Catalog, declared: &CatalogType) -> Expr {
    match catalog.value_shape_for_type(declared.id) {
        Some(CatalogValueShape::Scalar { .. }) => {
            public_scalar_expression(expression, declared.capabilities.wire)
        }
        Some(CatalogValueShape::DatabaseArray { element })
            if matches!(
                element.capabilities.wire,
                WireEncoding::BigInteger | WireEncoding::Numeric | WireEncoding::TextCast
            ) =>
        {
            expression.cast_as(Alias::new("text[]"))
        }
        Some(CatalogValueShape::DatabaseArray { .. }) | None => expression,
    }
}

fn public_aggregate_expression(
    expression: Expr,
    catalog: &Catalog,
    field: &crate::plan::AggregateProjection,
) -> Expr {
    if matches!(
        field.function,
        AggregateFunction::Min | AggregateFunction::Max
    ) && let Some(column) = field.operand
        && let Some(data_type) = catalog.type_for_column(column)
    {
        return public_catalog_expression(expression, catalog, data_type);
    }
    public_scalar_expression(expression, aggregate_wire(catalog, field))
}

fn aggregate_wire(catalog: &Catalog, field: &crate::plan::AggregateProjection) -> WireEncoding {
    if matches!(
        field.function,
        AggregateFunction::Min | AggregateFunction::Max
    ) && let Some(operand) = field.operand
        && let Some(data_type) = catalog.type_for_column(operand)
    {
        return data_type.capabilities.wire;
    }
    Catalog::builtin_capabilities(field.data_type).wire
}

fn text_cast_parameter(placeholder: &str, target: &TypeKey, collection: bool) -> String {
    let array = if collection { "[]" } else { "" };
    format!(
        "(({placeholder})::text{array})::{}{}",
        quoted_type_key(target),
        array
    )
}

fn dynamic_cast_before(target: Option<&TypeKey>) -> &'static str {
    if target.is_some() { "((" } else { "" }
}

fn dynamic_cast_after(target: Option<&TypeKey>, collection: bool) -> String {
    target.map_or_else(String::new, |target| {
        let array = if collection { "[]" } else { "" };
        format!(")::text{array})::{}{array}", quoted_type_key(target))
    })
}

fn quoted_type_key(key: &TypeKey) -> String {
    format!(
        "\"{}\".\"{}\"",
        key.schema.replace('"', "\"\""),
        key.name.replace('"', "\"\"")
    )
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
    relation: &Relation,
) -> Result<Condition, SqlGenerationError> {
    let mut condition = Condition::all();
    for (parent_column, child_column) in relation.local_columns.iter().zip(&relation.target_columns)
    {
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

fn catalog_relation(catalog: &Catalog, id: RelationId) -> Result<&Relation, SqlGenerationError> {
    catalog
        .relation_by_id(id)
        .ok_or(SqlGenerationError::MissingRelation(id.0))
}

fn table_label(table: &Table) -> String {
    format!("{}.{}", table.schema, table.name)
}

#[cfg(test)]
mod tests {
    use super::{DynamicMarker, DynamicMarkerKind, SqlGenerationError, replace_dynamic_markers};

    fn marker(kind: DynamicMarkerKind) -> DynamicMarker {
        DynamicMarker {
            internal: "__dsql_dynamic_1__".to_string(),
            public: "{{dynamic:0}}".to_string(),
            kind,
        }
    }

    #[test]
    fn structural_dynamic_marker_failures_are_not_retried() {
        let missing = replace_dynamic_markers("SELECT 1", &[marker(DynamicMarkerKind::Exact)]);
        assert!(matches!(
            missing,
            Err(SqlGenerationError::DynamicMarkerMissing(_))
        ));

        let missing_order = replace_dynamic_markers(
            "SELECT 1 ORDER BY __dsql_dynamic_1__",
            &[marker(DynamicMarkerKind::Order)],
        );
        assert!(matches!(
            missing_order,
            Err(SqlGenerationError::DynamicOrderMarkerRender(_))
        ));
    }

    #[test]
    fn colliding_dynamic_markers_request_a_fresh_sentinel() {
        let collision = replace_dynamic_markers(
            "SELECT '__dsql_dynamic_1__', __dsql_dynamic_1__",
            &[marker(DynamicMarkerKind::Exact)],
        );
        assert!(matches!(collision, Ok(None)));
    }
}
