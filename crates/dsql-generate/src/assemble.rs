//! Metadata assembly: settled facts become the per-operation and
//! per-fragment metadata documents host generators consume.

use std::path::Path;

use dsql_core::catalog::{Catalog, TableId};
use dsql_core::entities::aggregate::AggregateMode;
use dsql_core::entities::variable::VariableBinding;
use dsql_core::entities::variable_path::{is_input_path, is_params_path};
use dsql_core::facts::Span;
use dsql_core::plan::{
    CollectionPlan, CollectionResultPlan, FragmentPlanFact, OperationSeed, QueryPlan,
    SelectionPlan, SelectionPlanItem, SpreadUse,
};
use dsql_core::resolution::SelectionCardinality;
use dsql_core::sql::GeneratedSql;
use dsql_metadata::{
    DefinitionKind, DynamicInputMetadata, FragmentMetadata, FragmentSpreadMetadata, InputField,
    OperationMetadata, ResultDataType, ResultField, ResultFieldKind, ResultShape, SourceMapEntry,
    SourceRange, SqlDialect, SqlMetadata, SqlParameterMetadata, SqlVariantCaseMetadata,
    SqlVariantMetadata,
};

use crate::pipeline::{GenerateError, Result};

/// One operation's worth of scooped facts, ready to assemble.
pub(crate) struct OperationInputs<'a> {
    pub seed: &'a OperationSeed,
    pub plan: &'a QueryPlan,
    pub sql: &'a GeneratedSql,
    /// Variable bindings of the defining query, span-sorted.
    pub bindings: &'a [VariableBinding],
    /// The definition's source file, absolute as loaded.
    pub file: &'a str,
    /// Byte offset of the document inside its host file.
    pub source_offset: usize,
    /// For embedded documents: the content's byte range in the host.
    pub content_range: Option<SourceRange>,
}

/// One fragment's worth of scooped facts.
pub(crate) struct FragmentInputs<'a> {
    pub plan: &'a FragmentPlanFact,
    pub bindings: &'a [VariableBinding],
    pub file: &'a str,
    /// Byte offset of the document inside its host file.
    pub source_offset: usize,
    /// For embedded documents: the content's byte range in the host.
    pub content_range: Option<SourceRange>,
}

pub(crate) fn operation_metadata(
    catalog: &Catalog,
    source_root: Option<&Path>,
    inputs: &OperationInputs<'_>,
) -> Result<OperationMetadata> {
    Ok(OperationMetadata {
        name: operation_name(
            &inputs.seed.query_name,
            &inputs.plan.output_name,
            inputs.seed.root_count,
            inputs.seed.root_index,
        ),
        kind: DefinitionKind::Query.as_ref().to_string(),
        sql: SqlMetadata {
            dialect: SqlDialect::Postgres.as_ref().to_string(),
            text: inputs.sql.sql.clone(),
            parameters: inputs
                .sql
                .parameters
                .iter()
                .map(|parameter| SqlParameterMetadata {
                    path: parameter.path.clone(),
                })
                .collect(),
            variants: inputs
                .sql
                .variants
                .iter()
                .map(|variant| SqlVariantMetadata {
                    path: variant.path.clone(),
                    cases: variant
                        .cases
                        .iter()
                        .map(|case| SqlVariantCaseMetadata {
                            value: case.value.clone(),
                            text: case.text.clone(),
                        })
                        .collect(),
                })
                .collect(),
        },
        result: result_shape(catalog, inputs.plan)?,
        params: input_fields(inputs.bindings, true),
        input: input_fields(inputs.bindings, false),
        context: Vec::new(),
        dynamic_inputs: dynamic_inputs(inputs.bindings),
        policies: Vec::new(),
        handoffs: Vec::new(),
        fragment_spreads: fragment_spreads(&inputs.seed.spreads),
        source_map: vec![SourceMapEntry {
            id: inputs.seed.query_name.clone(),
            file: source_path(source_root, inputs.file),
            range: source_range(inputs.seed.def_span, inputs.source_offset),
            content_range: inputs.content_range,
        }],
    })
}

pub(crate) fn fragment_metadata(
    catalog: &Catalog,
    source_root: Option<&Path>,
    inputs: &FragmentInputs<'_>,
) -> Result<FragmentMetadata> {
    let table = catalog
        .table_by_id(inputs.plan.table)
        .ok_or_else(|| GenerateError::Assembly {
            name: inputs.plan.name.clone(),
            message: "missing fragment table".to_string(),
        })?;
    Ok(FragmentMetadata {
        name: inputs.plan.name.clone(),
        kind: DefinitionKind::Fragment.as_ref().to_string(),
        table: table.name.clone(),
        result: fragment_result_shape(
            catalog,
            inputs.plan.table,
            &inputs.plan.selections,
            &inputs.plan.name,
        )?,
        params: input_fields(inputs.bindings, true),
        input: input_fields(inputs.bindings, false),
        dynamic_inputs: dynamic_inputs(inputs.bindings),
        fragment_spreads: fragment_spreads(&inputs.plan.spreads),
        source_map: vec![SourceMapEntry {
            id: inputs.plan.name.clone(),
            file: source_path(source_root, inputs.file),
            range: source_range(inputs.plan.def_span, inputs.source_offset),
            content_range: inputs.content_range,
        }],
    })
}

fn fragment_spreads(spreads: &[SpreadUse]) -> Vec<FragmentSpreadMetadata> {
    spreads
        .iter()
        .map(|spread| FragmentSpreadMetadata {
            path: spread.path.clone(),
            fragment: spread.fragment.clone(),
        })
        .collect()
}

fn result_shape(catalog: &Catalog, plan: &QueryPlan) -> Result<ResultShape> {
    let mut fields = Vec::new();
    let nullable = collection_result_nullable(&plan.collection);
    if plan.flattened {
        collect_collection_children(
            catalog,
            plan.collection.table,
            "",
            &plan.collection.result,
            nullable,
            &mut fields,
        )?;
    } else {
        collect_collection_fields(
            catalog,
            "",
            &plan.output_name,
            &plan.collection,
            collection_result_kind(&plan.collection.result, plan.collection.shape.cardinality),
            nullable,
            &mut fields,
        )?;
    }
    Ok(ResultShape { fields })
}

fn fragment_result_shape(
    catalog: &Catalog,
    table: TableId,
    selection: &SelectionPlan,
    name: &str,
) -> Result<ResultShape> {
    let mut fields = Vec::new();
    for item in &selection.items {
        collect_result_item_fields(catalog, table, "", item, false, &mut fields)
            .map_err(|error| error.named(name))?;
    }
    Ok(ResultShape { fields })
}

fn collect_collection_fields(
    catalog: &Catalog,
    parent_path: &str,
    name: &str,
    collection: &CollectionPlan,
    kind: ResultFieldKind,
    nullable: bool,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    let path = join_path(parent_path, name);
    fields.push(ResultField {
        path: path.clone(),
        name: name.to_string(),
        parent_path: parent_path.to_string(),
        kind: kind.as_ref().to_string(),
        data_type: ResultDataType::Object.as_ref().to_string(),
        nullable,
    });

    collect_collection_children(
        catalog,
        collection.table,
        &path,
        &collection.result,
        false,
        fields,
    )?;
    Ok(())
}

fn collect_collection_children(
    catalog: &Catalog,
    current_table: TableId,
    parent_path: &str,
    result: &CollectionResultPlan,
    inherited_nullable: bool,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    match result {
        CollectionResultPlan::Rows(selection) => {
            for item in &selection.items {
                collect_result_item_fields(
                    catalog,
                    current_table,
                    parent_path,
                    item,
                    inherited_nullable,
                    fields,
                )?;
            }
        }
        CollectionResultPlan::Aggregate(aggregate) => {
            for key in &aggregate.group_keys {
                fields.push(ResultField {
                    path: join_path(parent_path, &key.output_name),
                    name: key.output_name.clone(),
                    parent_path: parent_path.to_string(),
                    kind: ResultFieldKind::Scalar.as_ref().to_string(),
                    data_type: key.data_type.as_str().to_string(),
                    nullable: inherited_nullable || key.nullable,
                });
            }
            for field in &aggregate.fields {
                fields.push(ResultField {
                    path: join_path(parent_path, &field.output_name),
                    name: field.output_name.clone(),
                    parent_path: parent_path.to_string(),
                    kind: ResultFieldKind::Scalar.as_ref().to_string(),
                    data_type: field.data_type.as_str().to_string(),
                    nullable: inherited_nullable || field.nullable,
                });
            }
        }
    }
    Ok(())
}

fn collect_result_item_fields(
    catalog: &Catalog,
    _current_table: TableId,
    parent_path: &str,
    item: &SelectionPlanItem,
    inherited_nullable: bool,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    match item {
        SelectionPlanItem::Projection(projection) => {
            let column =
                catalog
                    .column_by_id(projection.column)
                    .ok_or_else(|| GenerateError::Assembly {
                        name: String::new(),
                        message: "missing projected column".to_string(),
                    })?;
            fields.push(ResultField {
                path: join_path(parent_path, &projection.output_name),
                name: projection.output_name.clone(),
                parent_path: parent_path.to_string(),
                kind: ResultFieldKind::Scalar.as_ref().to_string(),
                data_type: column.data_type.as_str().to_string(),
                nullable: inherited_nullable || !column.not_null,
            });
        }
        SelectionPlanItem::Relation(relation) => {
            let related_table = relation.collection.table;
            let cardinality = relation.collection.shape.cardinality;
            let kind = collection_result_kind(&relation.collection.result, cardinality);
            let nullable = collection_result_nullable(&relation.collection);
            if relation.flattened {
                collect_collection_children(
                    catalog,
                    related_table,
                    parent_path,
                    &relation.collection.result,
                    inherited_nullable || nullable,
                    fields,
                )?;
            } else {
                collect_collection_fields(
                    catalog,
                    parent_path,
                    &relation.output_name,
                    &relation.collection,
                    kind,
                    inherited_nullable || nullable,
                    fields,
                )?;
            }
        }
    }
    Ok(())
}

fn collection_result_kind(
    result: &CollectionResultPlan,
    cardinality: SelectionCardinality,
) -> ResultFieldKind {
    match result {
        CollectionResultPlan::Aggregate(aggregate) => match aggregate.mode {
            AggregateMode::Ungrouped => ResultFieldKind::Object,
            AggregateMode::Grouped => ResultFieldKind::Array,
        },
        CollectionResultPlan::Rows(_) => match cardinality {
            SelectionCardinality::Collection => ResultFieldKind::Array,
            SelectionCardinality::AtMostOne => ResultFieldKind::Object,
        },
    }
}

fn collection_result_nullable(collection: &CollectionPlan) -> bool {
    matches!(collection.result, CollectionResultPlan::Rows(_))
        && collection.shape.cardinality == SelectionCardinality::AtMostOne
        && collection.shape.nullable
}

fn input_fields(bindings: &[VariableBinding], top_level: bool) -> Vec<InputField> {
    bindings
        .iter()
        .filter(|binding| {
            (top_level && is_params_path(&binding.path))
                || (!top_level && is_input_path(&binding.path))
        })
        .map(|binding| InputField {
            path: binding.path.clone(),
            data_type: binding.data_type.as_str().to_string(),
            collection: binding.collection.then_some(true),
            enum_values: binding.enum_values.clone(),
            required: true,
            nullable: false,
        })
        .collect()
}

fn dynamic_inputs(bindings: &[VariableBinding]) -> Vec<DynamicInputMetadata> {
    bindings
        .iter()
        .filter(|binding| !binding.enum_values.is_empty())
        .map(|binding| DynamicInputMetadata {
            name: binding.name.clone().unwrap_or_else(|| {
                binding
                    .path
                    .rsplit('.')
                    .next()
                    .unwrap_or("value")
                    .to_string()
            }),
            kind: binding.role.as_str().to_string(),
            preset: String::new(),
            fields: Vec::new(),
        })
        .collect()
}

pub(crate) fn operation_name(
    query_name: &str,
    output_name: &str,
    count: usize,
    index: usize,
) -> String {
    if count == 1 {
        query_name.to_string()
    } else {
        format!("{query_name}_{output_name}_{index}")
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

/// Definition spans are document-relative; the offset shifts them back to
/// host-file coordinates for embedded documents.
fn source_range(span: Span, source_offset: usize) -> SourceRange {
    SourceRange {
        start: (source_offset + span.start) as u32,
        end: (source_offset + span.end) as u32,
    }
}

pub(crate) fn source_path(source_root: Option<&Path>, file: &str) -> String {
    source_root
        .and_then(|root| Path::new(file).strip_prefix(root).ok())
        .map(|relative| relative.to_string_lossy().to_string())
        .unwrap_or_else(|| file.to_string())
}
