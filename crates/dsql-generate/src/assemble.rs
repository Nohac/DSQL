//! Metadata assembly: settled facts become the per-operation and
//! per-fragment metadata documents host generators consume.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dsql_core::catalog::{Catalog, TableId};
use dsql_core::entities::aggregate::AggregateMode;
use dsql_core::entities::variable::{
    InputDefault as CoreInputDefault, VariableBinding, VariableSource,
};
use dsql_core::entities::variable_path::{is_input_path, is_params_path};
use dsql_core::facts::Span;
use dsql_core::plan::{
    CollectionPlan, CollectionResultPlan, FragmentPlanFact, OperationSeed, PolicyAccess,
    PolicyApplicationPlan, PolicyAssignmentState, PolicyEnforcement, PolicyFieldAccess,
    PolicyFieldTarget, QueryPlan, QueryRootPlan, SelectionPlan, SelectionPlanItem, SpreadUse,
};
use dsql_core::resolution::SelectionCardinality;
use dsql_core::sql::GeneratedSql;
use dsql_metadata::{
    DefinitionKind, DynamicInputMetadata, FragmentMetadata, FragmentSpreadMetadata, InputDefault,
    InputField, OperationMetadata, PolicyApplicationMetadata, PolicyFieldAccessMetadata,
    PolicyMetadata, ResultDataType, ResultField, ResultFieldKind, ResultShape, SourceMapEntry,
    SourceRange, SqlDialect, SqlMetadata, SqlParameterMetadata, SqlVariantCaseMetadata,
    SqlVariantMetadata,
};

use crate::pipeline::{GenerateError, Result};

/// One operation's worth of scooped facts, ready to assemble.
pub(crate) struct OperationInputs<'a> {
    pub seed: &'a OperationSeed,
    pub plan: &'a QueryPlan,
    pub sql: &'a GeneratedSql,
    /// Effective variable bindings of the defining query, path-sorted.
    pub bindings: &'a [VariableBinding],
    /// The definition's source file, absolute as loaded.
    pub file: &'a str,
    /// Byte offset of the document inside its host file.
    pub source_offset: usize,
    /// For embedded documents: the content's byte range in the host.
    pub content_range: Option<SourceRange>,
    pub policy_sources: &'a [PolicySourceInput],
}

/// Declaration provenance retained only for operation policy metadata.
pub(crate) struct PolicySourceInput {
    pub entity: bowl::Entity,
    pub file: String,
    pub source_offset: usize,
    pub content_range: Option<SourceRange>,
    pub span: Span,
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
        name: inputs.seed.query_name.clone(),
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
                    null_text: variant.null_text.clone(),
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
        context: context_fields(
            inputs.bindings,
            &inputs.plan.policy_context,
            &inputs.sql.policy_context,
            &inputs.seed.query_name,
        )?,
        dynamic_inputs: dynamic_inputs(inputs.bindings),
        policies: policy_metadata(catalog, source_root, inputs)?,
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
            &inputs.plan.policy_nullable_fields,
            &inputs.plan.policy_field_access,
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

fn policy_metadata(
    catalog: &Catalog,
    source_root: Option<&Path>,
    inputs: &OperationInputs<'_>,
) -> Result<Vec<PolicyMetadata>> {
    let required_context = inputs
        .bindings
        .iter()
        .filter(|binding| binding.source == VariableSource::Context)
        .map(|binding| binding.path.as_str())
        .chain(
            inputs
                .plan
                .policy_context
                .iter()
                .map(|requirement| requirement.path.as_str()),
        )
        .chain(
            inputs
                .sql
                .policy_context
                .iter()
                .map(|requirement| requirement.path.as_str()),
        )
        .collect::<BTreeSet<_>>();
    let mut planned = Vec::new();
    for root in &inputs.plan.roots {
        collect_policy_applications(&root.collection, &mut planned);
    }
    let mut grouped = BTreeMap::<(String, String), Vec<&PolicyApplicationPlan>>::new();
    for application in planned {
        grouped
            .entry((
                application.identity.scope.clone(),
                application.identity.name.clone(),
            ))
            .or_default()
            .push(application);
    }

    let mut policies = Vec::new();
    for ((defined_in, name), mut applications) in grouped {
        applications.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.target.0.cmp(&right.target.0))
                .then_with(|| left.assignment.cmp(&right.assignment))
        });
        let Some(first) = applications.first().copied() else {
            continue;
        };
        let Some(source) = inputs
            .policy_sources
            .iter()
            .find(|source| source.entity == first.filter)
        else {
            return Err(GenerateError::Assembly {
                name: inputs.seed.query_name.clone(),
                message: format!("filter `{defined_in}::{name}` has no source provenance"),
            });
        };
        let mut context = applications
            .iter()
            .flat_map(|application| application.context.iter())
            .filter(|item| required_context.contains(item.path.as_str()))
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();
        context.sort();
        context.dedup();
        let mut conditions = first
            .conditions
            .iter()
            .map(|condition| format!("{}::{}", condition.scope, condition.name))
            .collect::<Vec<_>>();
        conditions.sort();
        conditions.dedup();

        let applications = applications
            .into_iter()
            .map(|application| policy_application_metadata(catalog, application))
            .collect::<Result<Vec<_>>>()?;
        policies.push(PolicyMetadata {
            scope: inputs.seed.scope.clone(),
            defined_in,
            name,
            default_active: first.default_active,
            enforcement: enforcement_label(first.enforcement).to_string(),
            conditions,
            context,
            source: SourceMapEntry {
                id: format!("{}::{}", first.identity.scope, first.identity.name),
                file: source_path(source_root, &source.file),
                range: source_range(source.span, source.source_offset),
                content_range: source.content_range,
            },
            applications,
        });
    }
    policies.sort_by(|left, right| {
        left.applications
            .first()
            .map(|application| application.path.as_str())
            .cmp(
                &right
                    .applications
                    .first()
                    .map(|application| application.path.as_str()),
            )
            .then_with(|| left.defined_in.cmp(&right.defined_in))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(policies)
}

fn collect_policy_applications<'a>(
    collection: &'a CollectionPlan,
    applications: &mut Vec<&'a PolicyApplicationPlan>,
) {
    applications.extend(&collection.policy_applications);
    if let CollectionResultPlan::Rows(selection) = &collection.result {
        for item in &selection.items {
            if let SelectionPlanItem::Relation(relation) = item {
                collect_policy_applications(&relation.collection, applications);
            }
        }
    }
}

fn policy_application_metadata(
    catalog: &Catalog,
    application: &PolicyApplicationPlan,
) -> Result<PolicyApplicationMetadata> {
    let table = catalog
        .table_by_id(application.target)
        .ok_or_else(|| GenerateError::Assembly {
            name: application.identity.name.clone(),
            message: "policy application target is missing from the catalog".to_string(),
        })?;
    let mut fields = application
        .fields
        .iter()
        .map(|field| {
            let (name, kind) = match field.target {
                PolicyFieldTarget::Column(column) => {
                    let column =
                        catalog
                            .column_by_id(column)
                            .ok_or_else(|| GenerateError::Assembly {
                                name: application.identity.name.clone(),
                                message: "policy field column is missing from the catalog"
                                    .to_string(),
                            })?;
                    (column.name.clone(), "column")
                }
                PolicyFieldTarget::Relation(relation_id) => {
                    let relation = catalog
                        .relation_fields_for_table(application.target)
                        .into_iter()
                        .find(|relation| relation.relation.id == relation_id)
                        .ok_or_else(|| GenerateError::Assembly {
                            name: application.identity.name.clone(),
                            message: "policy field relation is missing from the catalog"
                                .to_string(),
                        })?;
                    (relation.name.to_string(), "relation")
                }
            };
            Ok(PolicyFieldAccessMetadata {
                name,
                kind: kind.to_string(),
                access: access_label(field.access).to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    fields.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    Ok(PolicyApplicationMetadata {
        path: application.path.clone(),
        target: format!("{}.{}", table.schema, table.name),
        assignment: assignment_label(application.assignment).to_string(),
        rows_filtered: application.rows_filtered,
        fields,
    })
}

fn assignment_label(assignment: PolicyAssignmentState) -> &'static str {
    match assignment {
        PolicyAssignmentState::Default => "default",
        PolicyAssignmentState::Enabled => "enabled",
        PolicyAssignmentState::Disabled => "disabled",
        PolicyAssignmentState::Conditional => "conditional",
    }
}

fn enforcement_label(enforcement: PolicyEnforcement) -> &'static str {
    match enforcement {
        PolicyEnforcement::None => "none",
        PolicyEnforcement::Always => "always",
        PolicyEnforcement::Conditional => "conditional",
    }
}

#[derive(Clone, Copy)]
struct ResultAccessContext<'a> {
    inherited_nullable: bool,
    inherited_access: PolicyAccess,
    policy_nullable_fields: &'a [PolicyFieldTarget],
    policy_field_access: &'a [PolicyFieldAccess],
}

fn result_shape(catalog: &Catalog, plan: &QueryPlan) -> Result<ResultShape> {
    let mut fields = Vec::new();
    for root in &plan.roots {
        collect_root_result_fields(catalog, root, &mut fields)?;
    }
    Ok(ResultShape { fields })
}

fn collect_root_result_fields(
    catalog: &Catalog,
    plan: &QueryRootPlan,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    let nullable = collection_result_nullable(&plan.collection);
    let access = collection_policy_access(&plan.collection);
    if plan.flattened {
        collect_collection_children(
            catalog,
            plan.collection.table,
            "",
            &plan.collection.result,
            ResultAccessContext {
                inherited_nullable: nullable,
                inherited_access: access,
                policy_nullable_fields: &plan.collection.policy_nullable_fields,
                policy_field_access: &actual_field_access(&plan.collection),
            },
            fields,
        )?;
    } else {
        collect_collection_fields(
            catalog,
            "",
            &plan.output_name,
            &plan.collection,
            collection_result_kind(&plan.collection.result, plan.collection.shape.cardinality),
            ResultAccessContext {
                inherited_nullable: nullable,
                inherited_access: access,
                policy_nullable_fields: &[],
                policy_field_access: &[],
            },
            fields,
        )?;
    }
    Ok(())
}

fn fragment_result_shape(
    catalog: &Catalog,
    table: TableId,
    selection: &SelectionPlan,
    policy_nullable_fields: &[PolicyFieldTarget],
    policy_field_access: &[PolicyFieldAccess],
    name: &str,
) -> Result<ResultShape> {
    let mut fields = Vec::new();
    for item in &selection.items {
        collect_result_item_fields(
            catalog,
            table,
            "",
            item,
            ResultAccessContext {
                inherited_nullable: false,
                inherited_access: PolicyAccess::Unconditional,
                policy_nullable_fields,
                policy_field_access,
            },
            &mut fields,
        )
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
    access: ResultAccessContext<'_>,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    let path = join_path(parent_path, name);
    fields.push(ResultField {
        path: path.clone(),
        name: name.to_string(),
        parent_path: parent_path.to_string(),
        kind: kind.as_ref().to_string(),
        data_type: ResultDataType::Object.as_ref().to_string(),
        nullable: access.inherited_nullable,
        access: access_label(access.inherited_access).to_string(),
    });

    collect_collection_children(
        catalog,
        collection.table,
        &path,
        &collection.result,
        ResultAccessContext {
            inherited_nullable: false,
            inherited_access: PolicyAccess::Unconditional,
            policy_nullable_fields: &collection.policy_nullable_fields,
            policy_field_access: &actual_field_access(collection),
        },
        fields,
    )?;
    Ok(())
}

fn collect_collection_children(
    catalog: &Catalog,
    current_table: TableId,
    parent_path: &str,
    result: &CollectionResultPlan,
    access: ResultAccessContext<'_>,
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
                    access,
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
                    nullable: access.inherited_nullable
                        || key.nullable
                        || policy_filters_column(access.policy_nullable_fields, key.column),
                    access: access_label(access.inherited_access.combine(
                        policy_access_for_target(
                            access.policy_field_access,
                            PolicyFieldTarget::Column(key.column),
                        ),
                    ))
                    .to_string(),
                });
            }
            for field in &aggregate.fields {
                fields.push(ResultField {
                    path: join_path(parent_path, &field.output_name),
                    name: field.output_name.clone(),
                    parent_path: parent_path.to_string(),
                    kind: ResultFieldKind::Scalar.as_ref().to_string(),
                    data_type: field.data_type.as_str().to_string(),
                    nullable: access.inherited_nullable || field.nullable,
                    access: access_label(access.inherited_access.combine(field.operand.map_or(
                        PolicyAccess::Unconditional,
                        |column| {
                            policy_access_for_target(
                                access.policy_field_access,
                                PolicyFieldTarget::Column(column),
                            )
                        },
                    )))
                    .to_string(),
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
    access: ResultAccessContext<'_>,
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
                nullable: access.inherited_nullable
                    || !column.not_null
                    || policy_filters_column(access.policy_nullable_fields, projection.column),
                access: access_label(access.inherited_access.combine(policy_access_for_target(
                    access.policy_field_access,
                    PolicyFieldTarget::Column(projection.column),
                )))
                .to_string(),
            });
        }
        SelectionPlanItem::Relation(relation) => {
            let related_table = relation.collection.table;
            let cardinality = relation.collection.shape.cardinality;
            let kind = collection_result_kind(&relation.collection.result, cardinality);
            // A masked to-many relation is an empty array and a masked
            // aggregate keeps its non-null object/array shape. Only a
            // singular row relation becomes an absent object.
            let policy_nullable =
                policy_filters_relation(access.policy_nullable_fields, relation.relation)
                    && matches!(relation.collection.result, CollectionResultPlan::Rows(_))
                    && cardinality == SelectionCardinality::AtMostOne;
            let nullable = collection_result_nullable(&relation.collection) || policy_nullable;
            let field_access = access
                .inherited_access
                .combine(policy_access_for_target(
                    access.policy_field_access,
                    PolicyFieldTarget::Relation(relation.relation),
                ))
                .combine(collection_policy_access(&relation.collection));
            if relation.flattened {
                collect_collection_children(
                    catalog,
                    related_table,
                    parent_path,
                    &relation.collection.result,
                    ResultAccessContext {
                        inherited_nullable: access.inherited_nullable || nullable,
                        inherited_access: field_access,
                        policy_nullable_fields: &relation.collection.policy_nullable_fields,
                        policy_field_access: &actual_field_access(&relation.collection),
                    },
                    fields,
                )?;
            } else {
                collect_collection_fields(
                    catalog,
                    parent_path,
                    &relation.output_name,
                    &relation.collection,
                    kind,
                    ResultAccessContext {
                        inherited_nullable: access.inherited_nullable || nullable,
                        inherited_access: field_access,
                        policy_nullable_fields: &[],
                        policy_field_access: &[],
                    },
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
        && (collection.shape.nullable || collection.policy_filter.is_some())
}

fn collection_policy_access(collection: &CollectionPlan) -> PolicyAccess {
    collection
        .policy_filter
        .as_ref()
        .map_or(PolicyAccess::Unconditional, PolicyAccess::for_guard)
}

fn actual_field_access(collection: &CollectionPlan) -> Vec<PolicyFieldAccess> {
    collection
        .field_filters
        .iter()
        .map(|filter| PolicyFieldAccess {
            target: filter.target,
            access: PolicyAccess::for_guard(&filter.filter),
        })
        .collect()
}

fn policy_access_for_target(
    fields: &[PolicyFieldAccess],
    target: PolicyFieldTarget,
) -> PolicyAccess {
    fields
        .iter()
        .filter(|field| field.target == target)
        .fold(PolicyAccess::Unconditional, |access, field| {
            access.combine(field.access)
        })
}

fn access_label(access: PolicyAccess) -> &'static str {
    match access {
        PolicyAccess::Unconditional => "unconditional",
        PolicyAccess::ContextOnly => "context_only",
        PolicyAccess::RowDependent => "row_dependent",
    }
}

fn policy_filters_column(
    fields: &[PolicyFieldTarget],
    column: dsql_core::catalog::ColumnId,
) -> bool {
    fields.contains(&PolicyFieldTarget::Column(column))
}

fn policy_filters_relation(
    fields: &[PolicyFieldTarget],
    relation: dsql_core::catalog::RelationId,
) -> bool {
    fields.contains(&PolicyFieldTarget::Relation(relation))
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
            required: binding.required,
            nullable: binding.nullable,
            default: binding.default.as_ref().map(input_default),
        })
        .collect()
}

fn context_fields(
    bindings: &[VariableBinding],
    policy_context: &[dsql_core::plan::PolicyContextRequirement],
    rendered_policy_context: &[dsql_core::plan::PolicyContextRequirement],
    operation_name: &str,
) -> Result<Vec<InputField>> {
    let mut fields = BTreeMap::new();
    for binding in bindings
        .iter()
        .filter(|binding| binding.source == VariableSource::Context)
    {
        insert_context_field(
            &mut fields,
            operation_name,
            InputField {
                path: binding.path.clone(),
                data_type: binding.data_type.as_str().to_string(),
                collection: binding.collection.then_some(true),
                enum_values: binding.enum_values.clone(),
                required: true,
                nullable: false,
                default: None,
            },
        )?;
    }
    for requirement in policy_context.iter().chain(rendered_policy_context) {
        insert_context_field(
            &mut fields,
            operation_name,
            InputField {
                path: requirement.path.clone(),
                data_type: requirement.data_type.as_str().to_string(),
                collection: requirement.collection.then_some(true),
                enum_values: Vec::new(),
                required: true,
                nullable: false,
                default: None,
            },
        )?;
    }
    Ok(fields.into_values().collect())
}

fn input_default(default: &CoreInputDefault) -> InputDefault {
    match default {
        CoreInputDefault::String(value) => InputDefault {
            kind: "string".to_string(),
            value: Some(value.clone()),
            boolean: None,
            items: None,
        },
        CoreInputDefault::Number(value) => InputDefault {
            kind: "number".to_string(),
            value: Some(value.clone()),
            boolean: None,
            items: None,
        },
        CoreInputDefault::Boolean(value) => InputDefault {
            kind: "boolean".to_string(),
            value: None,
            boolean: Some(*value),
            items: None,
        },
        CoreInputDefault::Null => InputDefault {
            kind: "null".to_string(),
            value: None,
            boolean: None,
            items: None,
        },
        CoreInputDefault::Collection(items) => InputDefault {
            kind: "collection".to_string(),
            value: None,
            boolean: None,
            items: Some(items.iter().map(input_default).collect()),
        },
        CoreInputDefault::EmptyObject => InputDefault {
            kind: "empty_object".to_string(),
            value: None,
            boolean: None,
            items: None,
        },
    }
}

fn insert_context_field(
    fields: &mut BTreeMap<String, InputField>,
    operation_name: &str,
    field: InputField,
) -> Result<()> {
    if let Some(existing) = fields.get(&field.path) {
        if existing.data_type != field.data_type || existing.collection != field.collection {
            return Err(GenerateError::Assembly {
                name: operation_name.to_string(),
                message: format!(
                    "trusted context `{}` is required as incompatible `{}` and `{}` values",
                    field.path, existing.data_type, field.data_type
                ),
            });
        }
        return Ok(());
    }
    fields.insert(field.path.clone(), field);
    Ok(())
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
