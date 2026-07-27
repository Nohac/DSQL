//! Aggregate pipe-transform entity: syntax lowering, semantic checks, and
//! service contributions for collection summaries.

use bowl::{
    Commands, Component, DerivedFrom, Entity, In, Query, Registrar, SystemExt, Where, With,
};

use crate::catalog::{
    Catalog, CatalogSnapshot, ColumnId, DataType, FieldCheckResult, FieldRef, TableId, TableRef,
};
use crate::entities::clause::ClauseFact;
use crate::entities::expression::{Expr, build_expr};
use crate::entities::field_selection::FieldSel;
use crate::entities::{direct_name, direct_names, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, Children, DiagnosticCode, DiagnosticFacts, DiagnosticSource,
    DiagnosticsDemand, NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{NodeRef, Rule};
use crate::resolution::{
    FieldResolutions, ResolvedSelection, SelectionCardinality, SelectionTarget,
};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{
    CompletionContext, CompletionItem, CompletionKind, CompletionRequest, CompletionSite,
    emit_completion_candidate,
};
use crate::service::hover::{
    Cursor, HoverEnriched, describe_column, emit_hover_candidate, priority,
};
use crate::service::semantic_tokens::{SemanticToken, SemanticTokenKind, TokenChunk, TokensDemand};

/// One `source | transform { ... }` block. Ordered fields and group keys live
/// inside this tracked fact, like [`crate::entities::clause::OrderItem`] values
/// inside a clause fact. Its component fingerprint therefore changes for any
/// semantic subitem edit; spans participate too, causing harmless re-resolution
/// when an edit moves the block.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub struct AggregateTransformFact {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub group_keys: Vec<AggregateGroupKeySyntax>,
    pub fields: Vec<AggregateFieldSyntax>,
}

/// One optional-aliased group key after `aggregate by`.
#[derive(Debug, Clone, Hash)]
pub struct AggregateGroupKeySyntax {
    pub alias: Option<String>,
    pub alias_span: Option<Span>,
    pub path: Expr,
    pub span: Span,
}

/// One contextual aggregate function and optional direct-column operand.
#[derive(Debug, Clone, Hash)]
pub struct AggregateFieldSyntax {
    pub alias: Option<String>,
    pub alias_span: Option<Span>,
    pub function: String,
    pub function_span: Span,
    pub operand: Option<Expr>,
    pub span: Span,
}

/// The checked mode of one aggregate transform.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum AggregateMode {
    Ungrouped,
    Grouped,
}

/// A supported aggregate function after contextual-name resolution.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum AggregateFunction {
    Count,
    Exists,
    Min,
    Max,
    Sum,
    Avg,
}

/// One checked aggregate output field. Planning, SQL, metadata, and services
/// consume this semantic value without resolving its syntax again.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedAggregateField {
    pub function: Option<AggregateFunction>,
    pub function_span: Span,
    pub output_name: Option<String>,
    pub output_span: Span,
    pub operand: Option<ColumnId>,
    pub operand_span: Option<Span>,
    pub operand_name_span: Option<Span>,
    pub data_type: Option<DataType>,
    pub nullable: bool,
}

/// One aggregate scalar after contextual function and operand resolution.
/// Selection aggregates and predicate aggregates share this value so their
/// types, empty-input behavior, and provider allowlists cannot drift.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ResolvedAggregateValue {
    /// Contextual function name, when recognized.
    pub(crate) function: Option<AggregateFunction>,
    /// Direct operand column, when one resolved.
    pub(crate) operand: Option<ColumnId>,
    /// Whole operand path span.
    pub(crate) operand_span: Option<Span>,
    /// Terminal operand-name span used by editor services.
    pub(crate) operand_name_span: Option<Span>,
    /// Logical scalar result type.
    pub(crate) data_type: Option<DataType>,
    /// Whether this value may be SQL `NULL`.
    pub(crate) nullable: bool,
}

/// One checked direct scalar group key.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ResolvedAggregateGroupKey {
    pub output_name: String,
    pub output_span: Span,
    pub column: ColumnId,
    pub column_span: Span,
    pub data_type: DataType,
    pub nullable: bool,
}

/// The coherent semantic result for one pipe transform. A single component
/// carries ordered fields so every consumer observes the same resolution.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedAggregate {
    pub source: Entity,
    pub transform: Entity,
    pub table: Option<TableId>,
    pub mode: AggregateMode,
    pub group_keys: Vec<ResolvedAggregateGroupKey>,
    pub fields: Vec<ResolvedAggregateField>,
    pub problems: Vec<AggregateProblem>,
}

impl ResolvedAggregate {
    /// Whether the aggregate is safe for planning and artifact generation.
    pub fn is_valid(&self) -> bool {
        self.problems.is_empty()
    }
}

/// One stable validation failure found while producing [`ResolvedAggregate`].
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AggregateProblem {
    pub span: Span,
    pub kind: AggregateProblemKind,
}

/// Typed aggregate failure details shared by diagnostics and services.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum AggregateProblemKind {
    UnknownTransform {
        name: String,
    },
    SourceMustBeCollection {
        source: String,
    },
    PredicateSourceMustBeDirectCollection {
        source: String,
    },
    PredicateAggregateMustBeBoolean {
        function: AggregateFunction,
    },
    EmptyBody,
    InvalidGroupKey {
        written: String,
    },
    GroupedCannotFlatten,
    ExistsInGroupedAggregate,
    UnknownFunction {
        name: String,
    },
    MissingOperand {
        function: AggregateFunction,
    },
    UnexpectedOperand {
        function: AggregateFunction,
    },
    AliasRequired {
        function: AggregateFunction,
    },
    InvalidOperand {
        written: String,
    },
    UnsupportedOperandType {
        function: AggregateFunction,
        data_type: DataType,
    },
    DuplicateOutputKey {
        key: String,
    },
    OutputKeyTooLong {
        key: String,
        bytes: usize,
    },
}

/// Owns `pipe_transform` and consumes its aggregate subrules.
pub struct Aggregate;

impl LanguageEntity for Aggregate {
    const NAME: &'static str = "aggregate";

    fn register(reg: &mut Registrar<'_>) {
        reg.system(resolve_aggregates);
        reg.system(check_aggregates.run_during(bowl::Phase::Complete));
        reg.system(hover_aggregate_fields);
        reg.system(aggregate_tokens);
        reg.system(complete_aggregate_positions.run_during(bowl::Phase::Complete));
    }
}

impl LowerStage for Aggregate {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let name_span = direct_name(ctx.cst, node)?;
        let group_keys = ctx
            .cst
            .children(node)
            .filter(|child| ctx.cst.match_rule(*child, Rule::AggregateGroupKey))
            .filter_map(|group_key| lower_group_key(ctx, group_key))
            .collect();
        let fields = lower_fields(ctx, node);
        let fact = AggregateTransformFact {
            name: text(ctx.source, name_span).to_string(),
            name_span,
            span: node_span(ctx.cst, node),
            group_keys,
            fields,
        };
        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };
        Some(match ctx.parent {
            Some(parent) => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    fact,
                    ChildOf(parent),
                ))
                .untyped(),
            None => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    fact,
                ))
                .untyped(),
        })
    }
}

/// Normalizes the public keys named by an aggregate body while its owning
/// selection is lowered. Selection collision checks can then stay on their
/// existing tracked syntax input instead of ambiently reading semantic facts.
pub(crate) fn aggregate_output_keys(ctx: &LowerCtx<'_>, node: NodeRef) -> Vec<(String, Span)> {
    lower_fields(ctx, node)
        .into_iter()
        .filter_map(|field| {
            if let Some(alias) = field.alias {
                Some((alias, field.alias_span.unwrap_or(field.function_span)))
            } else if matches!(field.function.as_str(), "count" | "exists") {
                Some((field.function, field.function_span))
            } else {
                None
            }
        })
        .collect()
}

fn lower_fields(ctx: &LowerCtx<'_>, node: NodeRef) -> Vec<AggregateFieldSyntax> {
    direct_rule(ctx.cst, node, Rule::AggregateSet)
        .into_iter()
        .flat_map(|set| ctx.cst.children(set))
        .filter(|child| ctx.cst.match_rule(*child, Rule::AggregateField))
        .filter_map(|field| lower_field(ctx, field))
        .collect()
}

fn lower_group_key(ctx: &LowerCtx<'_>, node: NodeRef) -> Option<AggregateGroupKeySyntax> {
    let alias_span = direct_name(ctx.cst, node);
    let path = direct_rule(ctx.cst, node, Rule::ScopedPath)?;
    Some(AggregateGroupKeySyntax {
        alias: alias_span.map(|span| text(ctx.source, span).to_string()),
        alias_span,
        path: build_expr(ctx.cst, ctx.source, path),
        span: node_span(ctx.cst, node),
    })
}

fn lower_field(ctx: &LowerCtx<'_>, node: NodeRef) -> Option<AggregateFieldSyntax> {
    let names = direct_names(ctx.cst, node);
    let (alias_span, function_span) = match names.as_slice() {
        [function] => (None, *function),
        [alias, function, ..] => (Some(*alias), *function),
        [] => return None,
    };
    let operand = direct_rule(ctx.cst, node, Rule::ScopedPath)
        .map(|path| build_expr(ctx.cst, ctx.source, path));
    Some(AggregateFieldSyntax {
        alias: alias_span.map(|span| text(ctx.source, span).to_string()),
        alias_span,
        function: text(ctx.source, function_span).to_string(),
        function_span,
        operand,
        span: node_span(ctx.cst, node),
    })
}

/// Resolves one transform from tracked source-selection meaning and catalog
/// state. The transform fact already contains every ordered body item, so no
/// ambient gather can hide a dependency.
async fn resolve_aggregates(
    sources: Query<(
        Entity,
        &FieldSel,
        &Children,
        &FieldResolutions,
        &BelongsToFile,
    )>,
    source_resolution: Query<(Entity, &ResolvedSelection), Where<In<FieldResolutions>>>,
    transforms: Query<(Entity, &AggregateTransformFact), Where<In<Children>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::ResolvedAggregate,)>,
) {
    let (source_entity, source, _, _, file) = sources.item();
    let (resolution_entity, source_resolution) = source_resolution.item();
    let (transform_entity, transform) = transforms.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let mode = if transform.group_keys.is_empty() {
        AggregateMode::Ungrouped
    } else {
        AggregateMode::Grouped
    };
    let mut problems = Vec::new();
    if transform.name != "aggregate" {
        problems.push(AggregateProblem {
            span: transform.name_span,
            kind: AggregateProblemKind::UnknownTransform {
                name: transform.name.clone(),
            },
        });
    }
    if transform.fields.is_empty() {
        problems.push(AggregateProblem {
            span: transform.span,
            kind: AggregateProblemKind::EmptyBody,
        });
    }

    let (table, collection) = aggregate_source(source_resolution);
    if !collection && !matches!(&source_resolution.target, SelectionTarget::Unresolved) {
        problems.push(AggregateProblem {
            span: source.name_span,
            kind: AggregateProblemKind::SourceMustBeCollection {
                source: source.name.clone(),
            },
        });
    }

    let group_keys = table.map_or_else(Vec::new, |table| {
        transform
            .group_keys
            .iter()
            .filter_map(|key| resolve_group_key(catalog, table, key, &mut problems))
            .collect()
    });
    let fields = table.map_or_else(Vec::new, |table| {
        transform
            .fields
            .iter()
            .map(|field| resolve_field(catalog, table, mode, field, &mut problems))
            .collect()
    });
    if mode == AggregateMode::Grouped {
        if source.flattened {
            problems.push(AggregateProblem {
                span: source.name_span,
                kind: AggregateProblemKind::GroupedCannotFlatten,
            });
        }
        for field in &fields {
            if field.function == Some(AggregateFunction::Exists) {
                problems.push(AggregateProblem {
                    span: field.function_span,
                    kind: AggregateProblemKind::ExistsInGroupedAggregate,
                });
            }
        }
    }
    check_output_keys(
        group_keys
            .iter()
            .map(|key| (key.output_name.as_str(), key.output_span))
            .chain(fields.iter().filter_map(|field| {
                field
                    .output_name
                    .as_deref()
                    .map(|name| (name, field.output_span))
            })),
        &mut problems,
    );

    commands.insert((
        DerivedFrom::many([
            source_entity,
            resolution_entity,
            transform_entity,
            catalog_entity,
        ]),
        BelongsToFile(file.0),
        ResolvedAggregate {
            source: source_entity,
            transform: transform_entity,
            table,
            mode,
            group_keys,
            fields,
            problems,
        },
    ));
}

fn resolve_group_key(
    catalog: &Catalog,
    table: TableId,
    key: &AggregateGroupKeySyntax,
    problems: &mut Vec<AggregateProblem>,
) -> Option<ResolvedAggregateGroupKey> {
    let Expr::Path {
        anchor, segments, ..
    } = &key.path
    else {
        problems.push(AggregateProblem {
            span: key.span,
            kind: AggregateProblemKind::InvalidGroupKey {
                written: key.path.to_string(),
            },
        });
        return None;
    };
    let [segment] = segments.as_slice() else {
        problems.push(AggregateProblem {
            span: key.span,
            kind: AggregateProblemKind::InvalidGroupKey {
                written: key.path.to_string(),
            },
        });
        return None;
    };
    let reference = FieldRef {
        target: TableRef::parse(&segment.name),
        selector: None,
    };
    let FieldCheckResult::Column(column) = catalog.check_field_ref(table, reference) else {
        problems.push(AggregateProblem {
            span: key.span,
            kind: AggregateProblemKind::InvalidGroupKey {
                written: key.path.to_string(),
            },
        });
        return None;
    };
    if *anchor != crate::entities::expression::PathAnchor::Current
        || segment.relation_path.is_some()
        || segment.name.contains("::")
    {
        problems.push(AggregateProblem {
            span: key.span,
            kind: AggregateProblemKind::InvalidGroupKey {
                written: key.path.to_string(),
            },
        });
        return None;
    }
    Some(ResolvedAggregateGroupKey {
        output_name: key.alias.clone().unwrap_or_else(|| column.name.clone()),
        output_span: key.alias_span.unwrap_or(segment.span),
        column: column.id,
        column_span: segment.span,
        data_type: catalog.data_type_for_column(column.id),
        nullable: !column.not_null,
    })
}

fn aggregate_source(resolved: &ResolvedSelection) -> (Option<TableId>, bool) {
    let collection = resolved
        .shape
        .as_ref()
        .is_some_and(|shape| shape.cardinality == SelectionCardinality::Collection);
    match resolved.target {
        SelectionTarget::Table(table) | SelectionTarget::Relation { table, .. } => {
            (Some(table), collection)
        }
        SelectionTarget::Column(_) | SelectionTarget::Unresolved => (None, false),
    }
}

fn resolve_field(
    catalog: &Catalog,
    table: TableId,
    mode: AggregateMode,
    field: &AggregateFieldSyntax,
    problems: &mut Vec<AggregateProblem>,
) -> ResolvedAggregateField {
    let value = resolve_aggregate_value(
        catalog,
        table,
        mode,
        &field.function,
        field.function_span,
        field.operand.as_ref(),
        field.span,
        problems,
    );
    let output_span = field.alias_span.unwrap_or(field.function_span);
    let mut resolved = ResolvedAggregateField {
        function: value.function,
        function_span: field.function_span,
        output_name: field.alias.clone(),
        output_span,
        operand: value.operand,
        operand_span: value.operand_span,
        operand_name_span: value.operand_name_span,
        data_type: value.data_type,
        nullable: value.nullable,
    };
    let Some(function) = value.function else {
        return resolved;
    };

    match function {
        AggregateFunction::Count => {
            if field.operand.is_some() {
                require_alias(field, function, problems);
            } else {
                resolved.output_name = field.alias.clone().or_else(|| Some("count".to_string()));
            }
        }
        AggregateFunction::Exists => {
            resolved.output_name = field.alias.clone().or_else(|| Some("exists".to_string()));
        }
        AggregateFunction::Min
        | AggregateFunction::Max
        | AggregateFunction::Sum
        | AggregateFunction::Avg => {
            if field.operand.is_some() {
                require_alias(field, function, problems);
            }
        }
    }
    resolved
}

/// Resolves one aggregate scalar independently of its output alias or
/// placement. Callers add shape-specific rules after this shared contract.
#[expect(
    clippy::too_many_arguments,
    reason = "aggregate value resolution needs syntax spans and catalog context"
)]
pub(crate) fn resolve_aggregate_value(
    catalog: &Catalog,
    table: TableId,
    mode: AggregateMode,
    function_name: &str,
    function_span: Span,
    operand: Option<&Expr>,
    value_span: Span,
    problems: &mut Vec<AggregateProblem>,
) -> ResolvedAggregateValue {
    let function = match function_name {
        "count" => Some(AggregateFunction::Count),
        "exists" => Some(AggregateFunction::Exists),
        "min" => Some(AggregateFunction::Min),
        "max" => Some(AggregateFunction::Max),
        "sum" => Some(AggregateFunction::Sum),
        "avg" => Some(AggregateFunction::Avg),
        _ => {
            problems.push(AggregateProblem {
                span: function_span,
                kind: AggregateProblemKind::UnknownFunction {
                    name: function_name.to_string(),
                },
            });
            None
        }
    };
    let mut resolved = ResolvedAggregateValue {
        function,
        operand: None,
        operand_span: operand.map(Expr::span),
        operand_name_span: operand.and_then(|operand| match operand {
            Expr::Path { segments, .. } => segments.last().map(|segment| segment.span),
            _ => None,
        }),
        data_type: None,
        nullable: false,
    };
    let Some(function) = function else {
        return resolved;
    };

    match function {
        AggregateFunction::Count => {
            if let Some(operand) = operand {
                resolve_value_operand(catalog, table, operand, &mut resolved, problems);
            }
            resolved.data_type = Some(DataType::BigInt);
        }
        AggregateFunction::Exists => {
            resolved.data_type = Some(DataType::Boolean);
            if operand.is_some() {
                problems.push(AggregateProblem {
                    span: operand.map_or(value_span, Expr::span),
                    kind: AggregateProblemKind::UnexpectedOperand { function },
                });
            }
        }
        AggregateFunction::Min
        | AggregateFunction::Max
        | AggregateFunction::Sum
        | AggregateFunction::Avg => {
            let Some(operand) = operand else {
                problems.push(AggregateProblem {
                    span: function_span,
                    kind: AggregateProblemKind::MissingOperand { function },
                });
                return resolved;
            };
            resolve_value_operand(catalog, table, operand, &mut resolved, problems);
            let result_type = resolved
                .operand
                .and_then(|column| catalog.capabilities_for_column(column))
                .and_then(|capabilities| {
                    capabilities
                        .aggregates
                        .result(function, resolved.data_type?)
                });
            let supported = result_type.is_some();
            if let Some(data_type) = resolved.data_type
                && !supported
            {
                problems.push(AggregateProblem {
                    span: operand.span(),
                    kind: AggregateProblemKind::UnsupportedOperandType {
                        function,
                        data_type,
                    },
                });
            }
            if let Some(result_type) = result_type {
                resolved.data_type = Some(result_type);
            }
            resolved.nullable = mode == AggregateMode::Ungrouped
                || resolved
                    .operand
                    .and_then(|column| catalog.column_by_id(column))
                    .is_none_or(|column| !column.not_null);
        }
    }
    resolved
}

fn require_alias(
    field: &AggregateFieldSyntax,
    function: AggregateFunction,
    problems: &mut Vec<AggregateProblem>,
) {
    if field.alias.is_none() {
        problems.push(AggregateProblem {
            span: field.function_span,
            kind: AggregateProblemKind::AliasRequired { function },
        });
    }
}

fn resolve_value_operand(
    catalog: &Catalog,
    table: TableId,
    operand: &Expr,
    resolved: &mut ResolvedAggregateValue,
    problems: &mut Vec<AggregateProblem>,
) {
    let Expr::Path {
        anchor, segments, ..
    } = operand
    else {
        push_invalid_operand(operand, problems);
        return;
    };
    let [segment] = segments.as_slice() else {
        push_invalid_operand(operand, problems);
        return;
    };
    if *anchor != crate::entities::expression::PathAnchor::Current
        || segment.relation_path.is_some()
        || segment.name.contains("::")
    {
        push_invalid_operand(operand, problems);
        return;
    }
    let reference = FieldRef {
        target: TableRef::parse(&segment.name),
        selector: None,
    };
    let FieldCheckResult::Column(column) = catalog.check_field_ref(table, reference) else {
        push_invalid_operand(operand, problems);
        return;
    };
    resolved.operand = Some(column.id);
    resolved.data_type = Some(catalog.data_type_for_column(column.id));
}

fn push_invalid_operand(operand: &Expr, problems: &mut Vec<AggregateProblem>) {
    problems.push(AggregateProblem {
        span: operand.span(),
        kind: AggregateProblemKind::InvalidOperand {
            written: operand.to_string(),
        },
    });
}

fn check_output_keys<'a>(
    fields: impl IntoIterator<Item = (&'a str, Span)>,
    problems: &mut Vec<AggregateProblem>,
) {
    let mut seen = Vec::new();
    for (key, span) in fields {
        if seen.contains(&key) {
            problems.push(AggregateProblem {
                span,
                kind: AggregateProblemKind::DuplicateOutputKey {
                    key: key.to_string(),
                },
            });
        } else {
            seen.push(key);
        }
        let bytes = key.len();
        if bytes > crate::entities::field_selection::POSTGRES_RESULT_ALIAS_MAX_BYTES {
            problems.push(AggregateProblem {
                span,
                kind: AggregateProblemKind::OutputKeyTooLong {
                    key: key.to_string(),
                    bytes,
                },
            });
        }
    }
}

async fn check_aggregates(
    _: Query<Entity, With<DiagnosticsDemand>>,
    aggregates: Query<(Entity, &ResolvedAggregate, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (aggregate_entity, aggregate, file) = aggregates.item();
    for problem in &aggregate.problems {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::new(aggregate_entity),
                file: file.0,
                span: problem.span,
                severity: Severity::Error,
                source: DiagnosticSource::Check,
                code: problem.kind.code(),
                message: problem.kind.message(),
            },
        );
    }
}

/// Answers aggregate aliases, functions, and direct operands from the checked
/// semantic field rather than interpreting syntax at request time.
async fn hover_aggregate_fields(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    aggregates: Query<(Entity, &ResolvedAggregate), Where<bowl::Eq<BelongsToFile>>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = query.item();
    let (_, aggregate) = aggregates.item();
    let (_, snapshot) = catalog.item();
    for key in &aggregate.group_keys {
        let text = if key.column_span.contains(cursor.0) {
            describe_column(snapshot.catalog(), key.column)
        } else if key.output_span.contains(cursor.0) {
            Some(format!(
                "aggregate group key `{}`: {}{}",
                key.output_name,
                key.data_type.as_str(),
                if key.nullable { " (nullable)" } else { "" },
            ))
        } else {
            None
        };
        if let Some(text) = text {
            emit_hover_candidate(&mut commands, request, priority::FIELD, text);
        }
    }
    for field in &aggregate.fields {
        let text = if field
            .operand_name_span
            .is_some_and(|span| span.contains(cursor.0))
            && let Some(column) = field.operand
        {
            describe_column(snapshot.catalog(), column)
        } else if field.output_span != field.function_span && field.output_span.contains(cursor.0) {
            field.output_name.as_deref().map(|output| {
                format!(
                    "aggregate field `{output}`: {}{}",
                    field.data_type.map_or("unknown", DataType::as_str),
                    if field.nullable { " (nullable)" } else { "" },
                )
            })
        } else if field.function_span.contains(cursor.0) {
            field.function.map(|function| {
                format!(
                    "aggregate function `{}`: {}{}",
                    function.label(),
                    field.data_type.map_or("unknown", DataType::as_str),
                    if field.nullable { " (nullable)" } else { "" },
                )
            })
        } else {
            None
        };
        if let Some(text) = text {
            emit_hover_candidate(&mut commands, request, priority::FIELD, text);
        }
    }
}

/// Aggregate output aliases and resolved operands use the ordinary alias and
/// column token classes; contextual function names remain language keywords.
async fn aggregate_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    aggregates: Query<(Entity, &ResolvedAggregate, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::TokenChunk,)>,
) {
    let demand_entity = demand.item();
    let (aggregate_entity, aggregate, file) = aggregates.item();
    let mut tokens = Vec::new();
    for key in &aggregate.group_keys {
        if key.output_span != key.column_span {
            tokens.push(SemanticToken {
                span: key.output_span,
                kind: SemanticTokenKind::Alias,
            });
        }
        tokens.push(SemanticToken {
            span: key.column_span,
            kind: SemanticTokenKind::Column,
        });
    }
    for field in &aggregate.fields {
        if field.output_span != field.function_span && field.output_name.is_some() {
            tokens.push(SemanticToken {
                span: field.output_span,
                kind: SemanticTokenKind::Alias,
            });
        }
        if field.operand.is_some()
            && let Some(span) = field.operand_name_span
        {
            tokens.push(SemanticToken {
                span,
                kind: SemanticTokenKind::Column,
            });
        }
    }
    if !tokens.is_empty() {
        commands.insert((
            DerivedFrom::many([aggregate_entity, demand_entity]),
            BelongsToFile(file.0),
            TokenChunk(tokens),
        ));
    }
}

/// Offers contextual functions in an aggregate body and direct source-table
/// columns after the operand's `.` anchor.
async fn complete_aggregate_positions(
    requests: Query<(Entity, &CompletionContext), With<CompletionRequest>>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    let (request, context) = requests.item();
    if !matches!(
        context.site,
        CompletionSite::AggregateBody
            | CompletionSite::AggregateGroupKey
            | CompletionSite::PipeTransform
            | CompletionSite::PredicateAggregateFunction
            | CompletionSite::PredicateAggregateOperand
    ) {
        return;
    }
    let (_, snapshot) = catalog.item();
    let items = if context.site == CompletionSite::PipeTransform {
        vec![CompletionItem {
            label: "aggregate".to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("selection transform".to_string()),
            documentation: None,
            insert_text: None,
        }]
    } else if matches!(
        context.site,
        CompletionSite::AggregateGroupKey | CompletionSite::PredicateAggregateOperand
    ) || context.spread_dots > 0
    {
        context.table.map_or_else(Vec::new, |table| {
            snapshot
                .catalog()
                .columns_for_table(table)
                .map(|column| CompletionItem {
                    label: column.name.clone(),
                    kind: CompletionKind::Column,
                    detail: Some(
                        snapshot
                            .catalog()
                            .data_type_for_column(column.id)
                            .as_str()
                            .to_string(),
                    ),
                    documentation: column.description.clone(),
                    insert_text: (context.site == CompletionSite::AggregateGroupKey
                        && context.spread_dots == 0)
                        .then(|| format!(".{}", column.name)),
                })
                .collect()
        })
    } else {
        [
            AggregateFunction::Count,
            AggregateFunction::Exists,
            AggregateFunction::Min,
            AggregateFunction::Max,
            AggregateFunction::Sum,
            AggregateFunction::Avg,
        ]
        .into_iter()
        .map(|function| CompletionItem {
            label: function.label().to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("aggregate function".to_string()),
            documentation: None,
            insert_text: None,
        })
        .collect()
    };
    emit_completion_candidate(&mut commands, request, items);
}

impl AggregateProblemKind {
    pub(crate) fn code(&self) -> DiagnosticCode {
        match self {
            Self::UnknownTransform { .. } => DiagnosticCode::UnknownTransform,
            Self::SourceMustBeCollection { .. }
            | Self::PredicateSourceMustBeDirectCollection { .. } => {
                DiagnosticCode::AggregateSourceCardinality
            }
            Self::EmptyBody => DiagnosticCode::EmptyAggregate,
            Self::DuplicateOutputKey { .. } => DiagnosticCode::DuplicateOutputKey,
            Self::OutputKeyTooLong { .. } => DiagnosticCode::OutputKeyTooLong,
            Self::UnknownFunction { .. }
            | Self::InvalidGroupKey { .. }
            | Self::GroupedCannotFlatten
            | Self::ExistsInGroupedAggregate
            | Self::PredicateAggregateMustBeBoolean { .. }
            | Self::MissingOperand { .. }
            | Self::UnexpectedOperand { .. }
            | Self::AliasRequired { .. }
            | Self::InvalidOperand { .. }
            | Self::UnsupportedOperandType { .. } => DiagnosticCode::InvalidAggregateField,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::UnknownTransform { name } => format!("unknown selection transform `{name}`"),
            Self::SourceMustBeCollection { source } => format!(
                "aggregate source `{source}` must be a collection; singular and scalar sources cannot be aggregated"
            ),
            Self::PredicateSourceMustBeDirectCollection { source } => format!(
                "aggregate predicate source `{source}` must be a direct `.`-anchored collection relation"
            ),
            Self::PredicateAggregateMustBeBoolean { function } => format!(
                "aggregate function `{}` must be compared to a value; only `exists` is a predicate by itself",
                function.label()
            ),
            Self::EmptyBody => "aggregate body must contain at least one field".to_string(),
            Self::InvalidGroupKey { written } => format!(
                "aggregate group key `{written}` must be a `.`-anchored direct scalar column"
            ),
            Self::GroupedCannotFlatten => {
                "grouped aggregate output is array-valued and cannot be flattened".to_string()
            }
            Self::ExistsInGroupedAggregate => {
                "aggregate function `exists` is not meaningful in grouped output".to_string()
            }
            Self::UnknownFunction { name } => format!("unknown aggregate function `{name}`"),
            Self::MissingOperand { function } => {
                format!(
                    "aggregate function `{}` requires a direct column operand",
                    function.label()
                )
            }
            Self::UnexpectedOperand { function } => {
                format!(
                    "aggregate function `{}` does not accept an operand",
                    function.label()
                )
            }
            Self::AliasRequired { function } => format!(
                "aggregate function `{}` with an operand requires an output alias",
                function.label()
            ),
            Self::InvalidOperand { written } => {
                format!("aggregate operand `{written}` must be a `.`-anchored direct scalar column")
            }
            Self::UnsupportedOperandType {
                function,
                data_type,
            } => format!(
                "aggregate function `{}` does not support `{}` operands",
                function.label(),
                data_type.as_str()
            ),
            Self::DuplicateOutputKey { key } => {
                format!("aggregate output key `{key}` is ambiguous; use a distinct alias")
            }
            Self::OutputKeyTooLong { key, bytes } => format!(
                "aggregate output key `{key}` is {bytes} bytes; PostgreSQL result aliases must be at most {} bytes",
                crate::entities::field_selection::POSTGRES_RESULT_ALIAS_MAX_BYTES
            ),
        }
    }
}

impl AggregateFunction {
    /// The contextual source spelling of this function.
    pub fn label(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Exists => "exists",
            Self::Min => "min",
            Self::Max => "max",
            Self::Sum => "sum",
            Self::Avg => "avg",
        }
    }
}

/// Rejects source clauses whose semantics are not part of the first aggregate
/// contract. Ordinary clause checking still owns path and value diagnostics.
pub(crate) fn check_source_clause(
    ctx: &mut crate::entities::field_selection::CheckCtx<'_, '_>,
    clause_entity: Entity,
    clause: &ClauseFact,
    span: Span,
) {
    let label = match clause {
        ClauseFact::FilterAssignment { .. } | ClauseFact::Where { .. } => return,
        ClauseFact::OrderBy { .. } => "order by",
        ClauseFact::Limit { .. } => "limit",
        ClauseFact::Offset { .. } => "offset",
    };
    ctx.error(
        clause_entity,
        span,
        DiagnosticCode::AggregateClauseNotAllowed,
        format!("aggregate sources do not support `{label}` clauses"),
    );
}

impl FormatStage for Aggregate {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.aggregate_transform(node);
    }
}
