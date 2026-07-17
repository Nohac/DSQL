//! Aggregate pipe-transform entity: syntax lowering, semantic checks, and
//! service contributions for collection summaries.

use bowl::{
    Commands, Component, DerivedFrom, Entity, In, Query, Registrar, SystemExt, Where, With,
};

use crate::catalog::{
    Catalog, CatalogSnapshot, ColumnId, DataType, FieldCheckResult, FieldRef, RelationCardinality,
    TableId, TableRef,
};
use crate::entities::clause::ClauseFact;
use crate::entities::expression::{Expr, build_expr};
use crate::entities::field_selection::FieldSel;
use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, Children, DiagnosticCode, DiagnosticFacts, DiagnosticSource,
    DiagnosticsDemand, NodeKey, Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::resolution::{FieldResolutions, ResolvedSelection, SelectionTarget};
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

/// The coherent semantic result for one pipe transform. A single component
/// carries ordered fields so every consumer observes the same resolution.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedAggregate {
    pub source: Entity,
    pub transform: Entity,
    pub table: Option<TableId>,
    pub mode: AggregateMode,
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
    GroupedNotSupported,
    EmptyBody,
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
        let name_span = direct_token(ctx.cst, node, Token::Name)?;
        let group_keys = ctx
            .cst
            .children(node)
            .filter(|child| ctx.cst.match_rule(*child, Rule::AggregateGroupKey))
            .filter_map(|group_key| lower_group_key(ctx, group_key))
            .collect();
        let fields = direct_rule(ctx.cst, node, Rule::AggregateSet)
            .into_iter()
            .flat_map(|set| ctx.cst.children(set))
            .filter(|child| ctx.cst.match_rule(*child, Rule::AggregateField))
            .filter_map(|field| lower_field(ctx, field))
            .collect();
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

fn lower_group_key(ctx: &LowerCtx<'_>, node: NodeRef) -> Option<AggregateGroupKeySyntax> {
    let alias_span = direct_token(ctx.cst, node, Token::Name);
    let path = direct_rule(ctx.cst, node, Rule::ScopedPath)?;
    Some(AggregateGroupKeySyntax {
        alias: alias_span.map(|span| text(ctx.source, span).to_string()),
        alias_span,
        path: build_expr(ctx.cst, ctx.source, path),
        span: node_span(ctx.cst, node),
    })
}

fn lower_field(ctx: &LowerCtx<'_>, node: NodeRef) -> Option<AggregateFieldSyntax> {
    let names: Vec<Span> = ctx
        .cst
        .children(node)
        .filter_map(|child| ctx.cst.match_token(child, Token::Name).map(Span::from))
        .collect();
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
    if mode == AggregateMode::Grouped {
        problems.push(AggregateProblem {
            span: transform.span,
            kind: AggregateProblemKind::GroupedNotSupported,
        });
    }
    if transform.fields.is_empty() {
        problems.push(AggregateProblem {
            span: transform.span,
            kind: AggregateProblemKind::EmptyBody,
        });
    }

    let (table, collection) = aggregate_source(catalog, source_resolution);
    if !collection {
        problems.push(AggregateProblem {
            span: source.name_span,
            kind: AggregateProblemKind::SourceMustBeCollection {
                source: source.name.clone(),
            },
        });
    }

    let fields = table.map_or_else(Vec::new, |table| {
        transform
            .fields
            .iter()
            .map(|field| resolve_field(catalog, table, field, &mut problems))
            .collect()
    });
    check_output_keys(&fields, &mut problems);

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
            fields,
            problems,
        },
    ));
}

fn aggregate_source(catalog: &Catalog, resolved: &ResolvedSelection) -> (Option<TableId>, bool) {
    match resolved.target {
        SelectionTarget::Table(table) => (Some(table), true),
        SelectionTarget::Relation {
            table, foreign_key, ..
        } => {
            let cardinality = resolved.context.and_then(|context| {
                let foreign_key = catalog.foreign_key_by_id(foreign_key)?;
                catalog.relation_cardinality(context, table, foreign_key)
            });
            (
                Some(table),
                cardinality == Some(RelationCardinality::Collection),
            )
        }
        SelectionTarget::Column(_) | SelectionTarget::Unresolved => (None, false),
    }
}

fn resolve_field(
    catalog: &Catalog,
    table: TableId,
    field: &AggregateFieldSyntax,
    problems: &mut Vec<AggregateProblem>,
) -> ResolvedAggregateField {
    let function = match field.function.as_str() {
        "count" => Some(AggregateFunction::Count),
        "exists" => Some(AggregateFunction::Exists),
        "min" => Some(AggregateFunction::Min),
        "max" => Some(AggregateFunction::Max),
        _ => {
            problems.push(AggregateProblem {
                span: field.function_span,
                kind: AggregateProblemKind::UnknownFunction {
                    name: field.function.clone(),
                },
            });
            None
        }
    };
    let output_span = field.alias_span.unwrap_or(field.function_span);
    let mut resolved = ResolvedAggregateField {
        function,
        function_span: field.function_span,
        output_name: field.alias.clone(),
        output_span,
        operand: None,
        operand_span: field.operand.as_ref().map(Expr::span),
        operand_name_span: field.operand.as_ref().and_then(|operand| match operand {
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
            resolved.data_type = Some(DataType::Int);
            if let Some(operand) = &field.operand {
                resolve_operand(catalog, table, operand, &mut resolved, problems);
                // The operand controls which rows count, not the count's
                // public result type.
                resolved.data_type = Some(DataType::Int);
                require_alias(field, function, problems);
            } else {
                resolved.output_name = field.alias.clone().or_else(|| Some("count".to_string()));
            }
        }
        AggregateFunction::Exists => {
            resolved.data_type = Some(DataType::Boolean);
            resolved.output_name = field.alias.clone().or_else(|| Some("exists".to_string()));
            if field.operand.is_some() {
                problems.push(AggregateProblem {
                    span: field.operand.as_ref().map_or(field.span, Expr::span),
                    kind: AggregateProblemKind::UnexpectedOperand { function },
                });
            }
        }
        AggregateFunction::Min | AggregateFunction::Max => {
            let Some(operand) = &field.operand else {
                problems.push(AggregateProblem {
                    span: field.function_span,
                    kind: AggregateProblemKind::MissingOperand { function },
                });
                return resolved;
            };
            require_alias(field, function, problems);
            resolve_operand(catalog, table, operand, &mut resolved, problems);
            if let Some(data_type) = resolved.data_type
                && !matches!(
                    data_type,
                    DataType::Int | DataType::Text | DataType::Timestamptz
                )
            {
                problems.push(AggregateProblem {
                    span: operand.span(),
                    kind: AggregateProblemKind::UnsupportedOperandType {
                        function,
                        data_type,
                    },
                });
            }
            // An ungrouped min/max is null on an empty source even when the
            // operand column itself is not null.
            resolved.nullable = true;
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

fn resolve_operand(
    catalog: &Catalog,
    table: TableId,
    operand: &Expr,
    resolved: &mut ResolvedAggregateField,
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
    resolved.data_type = Some(column.data_type);
}

fn push_invalid_operand(operand: &Expr, problems: &mut Vec<AggregateProblem>) {
    problems.push(AggregateProblem {
        span: operand.span(),
        kind: AggregateProblemKind::InvalidOperand {
            written: operand.to_string(),
        },
    });
}

fn check_output_keys(fields: &[ResolvedAggregateField], problems: &mut Vec<AggregateProblem>) {
    let mut seen = Vec::new();
    for field in fields {
        let Some(key) = &field.output_name else {
            continue;
        };
        if seen.contains(key) {
            problems.push(AggregateProblem {
                span: field.output_span,
                kind: AggregateProblemKind::DuplicateOutputKey { key: key.clone() },
            });
        } else {
            seen.push(key.clone());
        }
        let bytes = key.len();
        if bytes > crate::entities::field_selection::POSTGRES_RESULT_ALIAS_MAX_BYTES {
            problems.push(AggregateProblem {
                span: field.output_span,
                kind: AggregateProblemKind::OutputKeyTooLong {
                    key: key.clone(),
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
    if context.site != CompletionSite::AggregateBody {
        return;
    }
    let (_, snapshot) = catalog.item();
    let items = if context.spread_dots > 0 {
        context.table.map_or_else(Vec::new, |table| {
            snapshot
                .catalog()
                .columns_for_table(table)
                .map(|column| CompletionItem {
                    label: column.name.clone(),
                    kind: CompletionKind::Column,
                    detail: Some(column.data_type.as_str().to_string()),
                    insert_text: None,
                })
                .collect()
        })
    } else {
        [
            AggregateFunction::Count,
            AggregateFunction::Exists,
            AggregateFunction::Min,
            AggregateFunction::Max,
        ]
        .into_iter()
        .map(|function| CompletionItem {
            label: function.label().to_string(),
            kind: CompletionKind::Keyword,
            detail: Some("aggregate function".to_string()),
            insert_text: None,
        })
        .collect()
    };
    emit_completion_candidate(&mut commands, request, items);
}

impl AggregateProblemKind {
    fn code(&self) -> DiagnosticCode {
        match self {
            Self::UnknownTransform { .. } => DiagnosticCode::UnknownTransform,
            Self::SourceMustBeCollection { .. } => DiagnosticCode::AggregateSourceCardinality,
            Self::GroupedNotSupported => DiagnosticCode::GroupedAggregateUnsupported,
            Self::EmptyBody => DiagnosticCode::EmptyAggregate,
            Self::DuplicateOutputKey { .. } => DiagnosticCode::DuplicateOutputKey,
            Self::OutputKeyTooLong { .. } => DiagnosticCode::OutputKeyTooLong,
            Self::UnknownFunction { .. }
            | Self::MissingOperand { .. }
            | Self::UnexpectedOperand { .. }
            | Self::AliasRequired { .. }
            | Self::InvalidOperand { .. }
            | Self::UnsupportedOperandType { .. } => DiagnosticCode::InvalidAggregateField,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::UnknownTransform { name } => format!("unknown selection transform `{name}`"),
            Self::SourceMustBeCollection { source } => format!(
                "aggregate source `{source}` must be a collection; singular and scalar sources cannot be aggregated"
            ),
            Self::GroupedNotSupported => {
                "grouped aggregates are recognized but are not implemented yet".to_string()
            }
            Self::EmptyBody => "aggregate body must contain at least one field".to_string(),
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
        ClauseFact::Where { .. } => return,
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
