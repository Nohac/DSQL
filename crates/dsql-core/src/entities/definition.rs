//! Definition entity: named top-level definitions (queries and fragments),
//! the definition index, and the duplicate-fragment check.
//!
//! Queries and fragments are one entity because they are structurally the
//! same concept — a named definition with a selection set — and every stage
//! treats them symmetrically except where [`DefKind`] branches.

use crate::schema::{AstFacts, dsql_schema};
use std::{collections::BTreeMap, fmt};

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Phase, Query, Registrar, Singleton,
    SystemExt, TrackedView, View, Where, With,
};

use crate::entities::variable::{
    DefinitionVariables, InputRefinement, VariableBinding, VariableRole, build_input_refinements,
    input_default_label, variable_type_label,
};
use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::parser::{NodeRef, Rule};
use crate::plan::{
    DynamicInputContract, DynamicInputKind, DynamicPredicateOperator, QueryPlanFact,
};
use crate::resolution::{ResolvedFragmentTarget, ResolvedTableTarget};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::source::{ResolutionScope, ScopeImports, SourceText};

/// What kind of definition a [`DefDecl`] fact describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DefKind {
    Query,
    Fragment,
}

impl fmt::Display for DefKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefKind::Query => f.write_str("query"),
            DefKind::Fragment => f.write_str("fragment"),
        }
    }
}

/// One named top-level definition, lowered from `query_def`/`fragment_def`.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub struct DefDecl {
    pub kind: DefKind,
    pub name: String,
    /// Span of the name token, for name-precision diagnostics.
    pub name_span: Span,
    /// Span of the whole definition.
    pub span: Span,
    /// Fingerprint of the definition's source slice. The check, variable,
    /// and plan walks read the definition *body* through ambient views;
    /// this hash is the tracked dependency that re-runs them on any body
    /// edit — including same-length edits that move no span.
    pub source_hash: u64,
    /// Contract refinements written in the definition header.
    pub input_refinements: Vec<InputRefinement>,
}

/// The relation a fragment is declared `on`. Only fragment entities carry
/// this; the catalog check (phase 6) validates it against the schema.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct FragmentTarget {
    pub name: String,
    pub span: Span,
}

/// Join key carried by fragment definitions and fragment spreads alike, so
/// spread resolution is a bound join on the name (see `fragment_spread`).
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct FragmentKey(pub String);

/// Fingerprint of the full definition set (scope, kind, name), maintained
/// by [`index_defs`]. Checks that must react to *other* definitions
/// appearing, disappearing, or changing scope take this singleton as a
/// tracked input: its revision moves only when the set actually changes,
/// so idempotent reruns invalidate nothing. Fragment entries carry their
/// defining file's content fingerprint: fragment *bodies* are expanded
/// ambiently by the check/variable/plan walks, and this fingerprint is
/// the tracked dependency that re-runs dependents when a fragment's
/// content — not just its name — changes. Query entries stay name-only,
/// so ordinary query edits never invalidate unrelated definitions.
#[derive(Component, Hash)]
#[component(hash)]
pub struct DefIndex(Vec<(String, DefKind, String, Option<u64>)>);

impl DefIndex {
    pub(crate) fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// Owns `query_def` and `fragment_def`.
pub struct Definition;

impl LanguageEntity for Definition {
    const NAME: &'static str = "definition";

    fn register(reg: &mut Registrar<'_>) {
        // Ambient readers of lowered facts sit behind the Complete phase
        // barrier (the engine's same-phase race flag enforces this);
        // check_fragment_targets reads only tracked inputs and needs none.
        reg.system(index_defs.run_during(Phase::Complete));
        reg.system(check_duplicate_definitions.run_during(Phase::Complete));
        reg.system(check_import_collisions.run_during(Phase::Complete));
        reg.system(check_import_ambiguities.run_during(Phase::Complete));
        reg.system(check_fragment_targets);
        // Fully tracked (per-file and per-definition bound joins, no views),
        // so it needs no phase barrier: replanning orders it after enrichment,
        // variable inference, and the optional plan capability contract.
        reg.system(hover_definitions);
    }
}

/// A fragment's `on` target must resolve to a catalog table; its body is
/// only checked once it does (see the field-selection check systems).
async fn check_fragment_targets(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &ResolvedFragmentTarget, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (resolution, target, file) = query.item();

    match &target.target {
        ResolvedTableTarget::Table(_) => {}
        ResolvedTableTarget::NotFound { reference } => {
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::new(resolution),
                    file: file.0,
                    span: target.span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::TableNotFound,
                    message: format!("table `{reference}` not found"),
                },
            );
        }
        ResolvedTableTarget::Ambiguous {
            reference,
            candidates,
        } => {
            let candidates: Vec<String> = candidates
                .iter()
                .map(|key| format!("{}::{}", key.schema, key.table))
                .collect();
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::new(resolution),
                    file: file.0,
                    span: target.span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::AmbiguousTable,
                    message: format!(
                        "table `{reference}` is ambiguous; use an alias with a schema-qualified name ({})",
                        candidates.join(", ")
                    ),
                },
            );
        }
    }
}

impl LowerStage for Definition {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        // The name is a direct child token; nested Names (inside the
        // selection set) belong to other entities.
        let Some(name_span) = direct_name(ctx.cst, node) else {
            // Error recovery can leave a def without a name; the parse
            // diagnostics already cover it.
            return None;
        };

        let kind = if ctx.cst.match_rule(node, Rule::QueryDef) {
            DefKind::Query
        } else {
            DefKind::Fragment
        };

        let span = node_span(ctx.cst, node);
        let source_hash = crate::source::content_hash(text(ctx.source, span));
        let header_rule = match kind {
            DefKind::Query => Rule::QueryHeader,
            DefKind::Fragment => Rule::FragmentHeader,
        };
        let input_refinements = direct_rule(ctx.cst, node, header_rule)
            .map(|header| build_input_refinements(ctx.cst, ctx.source, header))
            .unwrap_or_default();
        let decl = DefDecl {
            kind,
            name: text(ctx.source, name_span).to_string(),
            name_span,
            span,
            source_hash,
            input_refinements,
        };

        let target = direct_rule(ctx.cst, node, Rule::QualifiedName).map(|target| {
            let span = node_span(ctx.cst, target);
            FragmentTarget {
                name: text(ctx.source, span).to_string(),
                span,
            }
        });

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let scope = ResolutionScope(ctx.scope.to_string());
        let entity = match (kind, target) {
            (DefKind::Fragment, Some(target)) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                scope,
                FragmentKey(decl.name.clone()),
                decl,
                target,
            )),
            // A fragment whose `on` target was lost to error recovery still
            // lowers (spreads may resolve to it); the parse diagnostics
            // already report the malformed target.
            (DefKind::Fragment, None) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                scope,
                FragmentKey(decl.name.clone()),
                decl,
            )),
            (DefKind::Query, _) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                scope,
                decl,
            )),
        };
        Some(entity.untyped())
    }
}

/// Aggregates the definition set into the [`DefIndex`] singleton. The tracked
/// views form one zero-key invocation over the complete owner set, so adding or
/// removing a source reruns the same derived owner instead of letting per-file
/// invocations compete over one singleton. Equal recomputation is
/// fingerprint-neutral.
///
/// Ungated: spread resolution and planning consume it, not just diagnostics.
/// Runs at Complete, behind the phase barrier its lowered facts need;
/// index-tracked consumers replan when it commits.
async fn index_defs(
    defs: TrackedView<'_, (Entity, &DefDecl, &ResolutionScope, &BelongsToFile)>,
    files: TrackedView<'_, (Entity, &SourceText)>,
    mut commands: Commands<(dsql_schema::DefIndex,)>,
) {
    let file_hashes: std::collections::HashMap<Entity, u64> = files
        .iter()
        .map(|(entity, text)| (entity, text.content_hash()))
        .collect();
    let mut entries: Vec<(String, DefKind, String, Option<u64>)> = defs
        .iter()
        .map(|(_, decl, scope, file)| {
            // Fragment bodies are walk-expanded across files; their file
            // fingerprint is the dependency that keeps dependents fresh.
            let body = (decl.kind == DefKind::Fragment)
                .then(|| file_hashes.get(&file.0).copied().unwrap_or_default());
            (scope.0.clone(), decl.kind, decl.name.clone(), body)
        })
        .collect();
    entries.sort();

    commands.insert((Singleton::<DefIndex>::new(), DefIndex(entries)));
}

/// Duplicate names of the same definition kind are errors within one
/// resolution scope. Fragments would be ambiguous at spread-resolution time;
/// operations would collide at the generation artifact boundary. The same
/// name in independent scopes remains valid, and local-vs-imported and
/// import-vs-import collisions have their own checks below.
///
/// The [`DefIndex`] query keeps this check honest: the `View` of other
/// definitions contributes no memo deps, so without a tracked input over the
/// definition *set*, a row would never rerun when an unrelated definition is
/// added or removed — a surviving duplicate could go unreported.
async fn check_duplicate_definitions(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    _index: Query<(Entity, &DefIndex)>,
    defs: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, decl, file, scope) = query.item();

    let Some((previous, _, _)) = defs.iter().find(|(other, other_decl, other_scope)| {
        *other < entity
            && other_decl.kind == decl.kind
            && other_decl.name == decl.name
            && other_scope.0 == scope.0
    }) else {
        return;
    };

    let noun = match decl.kind {
        DefKind::Query => "operation",
        DefKind::Fragment => "fragment",
    };

    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::many([entity, previous]),
            file: file.0,
            span: decl.name_span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code: DiagnosticCode::DuplicateDefinition,
            message: format!("duplicate {noun} `{}`", decl.name),
        },
    );
}

/// A local definition (either kind) whose name is also provided by an
/// imported scope is a diagnostic at the local definition
/// (docs/spec/resolution-scopes.md).
async fn check_import_collisions(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    _index: Query<(Entity, &DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    defs: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, decl, file, scope) = query.item();
    let (_, imports) = imports.item();

    let Some((imported, _, imported_scope)) = defs
        .iter()
        .filter(|(_, other_decl, other_scope)| {
            other_decl.kind == decl.kind
                && other_decl.name == decl.name
                && imports
                    .imports_of(&scope.0)
                    .any(|import| import == other_scope.0)
        })
        .min_by(
            |(left_entity, _, left_scope), (right_entity, _, right_scope)| {
                left_scope
                    .0
                    .cmp(&right_scope.0)
                    .then_with(|| left_entity.cmp(right_entity))
            },
        )
    else {
        return;
    };

    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::many([entity, imported]),
            file: file.0,
            span: decl.name_span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code: DiagnosticCode::DuplicateDefinition,
            message: format!(
                "{} `{}` collides with a definition imported from scope `{}`",
                decl.kind, decl.name, imported_scope.0
            ),
        },
    );
}

/// Two imported scopes providing one query name to the same consuming
/// scope collide in that scope's artifact closure, with no local
/// definition to anchor a diagnostic — this reports once, on the
/// lexicographically first provider, naming every provider scope.
/// Fragments deliberately keep their *spread-site* ambiguity diagnostic
/// instead (two importable fragments with one name are harmless until a
/// spread actually uses the name); queries have no use sites, so the
/// definition level is the only place to say it.
async fn check_import_ambiguities(
    _: Query<Entity, With<DiagnosticsDemand>>,
    imports: Query<(Entity, &ScopeImports)>,
    // The definition index is the tracked input that re-runs this check
    // when definitions appear, rename, or vanish; the ambient view alone
    // would go stale after the first settle.
    _index: Query<(Entity, &DefIndex)>,
    defs: View<'_, (Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (_, imports) = imports.item();

    for consumer in imports.0.keys() {
        // Group this consumer's imported queries by name.
        let mut providers: std::collections::BTreeMap<&str, Vec<(Entity, &DefDecl, Entity, &str)>> =
            std::collections::BTreeMap::new();
        for (entity, decl, file, def_scope) in defs.iter() {
            if decl.kind == DefKind::Query
                && imports
                    .imports_of(consumer)
                    .any(|import| import == def_scope.0)
            {
                providers.entry(decl.name.as_str()).or_default().push((
                    entity,
                    decl,
                    file.0,
                    def_scope.0.as_str(),
                ));
            }
        }
        for (name, mut group) in providers {
            let distinct: std::collections::BTreeSet<&str> =
                group.iter().map(|(_, _, _, scope)| *scope).collect();
            if distinct.len() < 2 {
                continue;
            }
            group.sort_by_key(|(_, decl, _, scope)| (*scope, decl.name_span.start));
            let (_, decl, file, _) = group[0];
            let scopes = distinct.into_iter().collect::<Vec<_>>().join("`, `");
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::many(group.iter().map(|(entity, _, _, _)| *entity)),
                    file,
                    span: decl.name_span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::DuplicateDefinition,
                    message: format!(
                        "query `{name}` is provided to scope `{consumer}` by scopes `{scopes}`"
                    ),
                },
            );
        }
    }
}

impl FormatStage for Definition {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        if formatter.rule(node) == Some(Rule::QueryDef) {
            formatter.write_str("query");
            if let Some(name) = formatter.direct_name_text(node) {
                formatter.write_str(" ");
                formatter.write_str(&name);
            }
            if let Some(header) = formatter.direct_rule(node, Rule::QueryHeader) {
                formatter.definition_header(header);
            }
            for directive in formatter.direct_rules(node, Rule::Directive) {
                formatter.format_child(directive);
            }
        } else {
            formatter.write_str("fragment");
            if let Some(name) = formatter.direct_name_text(node) {
                formatter.write_str(" ");
                formatter.write_str(&name);
            }
            if let Some(header) = formatter.direct_rule(node, Rule::FragmentHeader) {
                formatter.definition_header(header);
            }
            formatter.write_str(" on");
            if let Some(on) = formatter.direct_qualified_name_text(node) {
                formatter.write_str(" ");
                formatter.write_str(&on);
            }
        }
        if let Some(selection_set) = formatter.direct_rule(node, Rule::SelectionSet) {
            formatter.selection_set(selection_set);
        }
    }
}

/// Answers hover on a definition name with its kind and target: one
/// invocation per (request, definition-in-file) pair via the
/// `BelongsToFile` join, the fragment target riding the definition row as
/// an optional part.
/// One definition row in the hovered file: the declaration with its
/// optional fragment target riding along.
type DefInFile<'a> = (Entity, &'a DefDecl, &'a NodeKey, Option<&'a FragmentTarget>);

/// Optional variable aggregate for the definition row currently paired by
/// [`NodeKey`]. It is absent when variable analysis was not demanded.
type VariablesForDefinition<'a> =
    Option<Query<(Entity, &'a DefinitionVariables), Where<BowlEq<NodeKey>>>>;

/// Optional query plan for the definition row currently paired by [`NodeKey`].
/// It is absent when plan demand is not armed or the query cannot be planned.
type PlanForDefinition<'a> = Option<Query<(Entity, &'a QueryPlanFact), Where<BowlEq<NodeKey>>>>;

async fn hover_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    defs: Query<DefInFile<'_>, Where<BowlEq<BelongsToFile>>>,
    variables: VariablesForDefinition<'_>,
    plan: PlanForDefinition<'_>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_def_entity, decl, _key, target) = defs.item();

    if !decl.name_span.contains(cursor.0) {
        return;
    }

    let dynamic_inputs = plan
        .map(|plan| plan.item().1.0.dynamic_inputs.as_slice())
        .unwrap_or_default();
    let (priority, text) = match (decl.kind, target, variables) {
        (DefKind::Query, _, Some(variables)) => {
            let (_, variables) = variables.item();
            (
                priority::QUERY_SIGNATURE,
                describe_query_variables(&decl.name, variables, dynamic_inputs),
            )
        }
        (DefKind::Query, _, None) => (priority::DEFINITION, format!("query `{}`", decl.name)),
        (DefKind::Fragment, Some(target), _) => (
            priority::DEFINITION,
            format!("fragment `{}` on `{}`", decl.name, target.name),
        ),
        (DefKind::Fragment, None, _) => (priority::DEFINITION, format!("fragment `{}`", decl.name)),
    };

    emit_hover_candidate(&mut commands, request, priority, text);
}

fn describe_query_variables(
    name: &str,
    variables: &DefinitionVariables,
    dynamic_inputs: &[DynamicInputContract],
) -> String {
    let shape = variable_shape(&variables.bindings, dynamic_inputs);
    if shape.is_empty() {
        format!("### Query `{name}`\n\nNo variables.")
    } else {
        format!("### Query `{name}`\n\n#### Variables\n\n```yaml\n{shape}```")
    }
}

#[derive(Default)]
struct VariableShapeNode {
    children: BTreeMap<String, VariableShapeNode>,
    value: Option<VariableShapeValue>,
}

enum VariableShapeValue {
    Scalar(String),
    Dynamic {
        binding: Box<VariableBinding>,
        contract: DynamicInputContract,
    },
}

fn variable_shape(bindings: &[VariableBinding], dynamic_inputs: &[DynamicInputContract]) -> String {
    let mut root = VariableShapeNode::default();
    for binding in bindings {
        let value = dynamic_contract(binding, dynamic_inputs).map_or_else(
            || VariableShapeValue::Scalar(variable_type_label(binding)),
            |contract| VariableShapeValue::Dynamic {
                binding: Box::new(binding.clone()),
                contract: contract.clone(),
            },
        );
        insert_variable_shape(
            &mut root,
            &binding.path.split('.').collect::<Vec<_>>(),
            value,
        );
    }
    let mut output = String::new();
    render_variable_shape(&root, 0, &mut output);
    output
}

fn dynamic_contract<'a>(
    binding: &VariableBinding,
    dynamic_inputs: &'a [DynamicInputContract],
) -> Option<&'a DynamicInputContract> {
    let kind = match binding.role {
        VariableRole::DynamicPredicate => DynamicInputKind::Predicate,
        VariableRole::DynamicOrder => DynamicInputKind::Order,
        _ => return None,
    };
    dynamic_inputs
        .iter()
        .find(|contract| contract.path == binding.path && contract.kind == kind)
}

pub(crate) fn describe_dynamic_variable(
    binding: &VariableBinding,
    dynamic_inputs: &[DynamicInputContract],
    binding_time: &str,
) -> Option<String> {
    dynamic_contract(binding, dynamic_inputs)?;
    let label = binding
        .name
        .as_deref()
        .map(|name| format!("`{name}`"))
        .unwrap_or_else(|| "anonymous variable".to_string());
    let shape = variable_shape(std::slice::from_ref(binding), dynamic_inputs);
    Some(format!(
        "{label} — `{}` ({binding_time})\n\n```yaml\n{shape}```",
        binding.path
    ))
}

fn insert_variable_shape(node: &mut VariableShapeNode, path: &[&str], value: VariableShapeValue) {
    let Some((head, tail)) = path.split_first() else {
        node.value = Some(value);
        return;
    };
    insert_variable_shape(
        node.children.entry((*head).to_string()).or_default(),
        tail,
        value,
    );
}

fn render_variable_shape(node: &VariableShapeNode, indent: usize, output: &mut String) {
    for (key, child) in &node.children {
        output.push_str(&"  ".repeat(indent));
        output.push_str(key);
        if child.children.is_empty() {
            match &child.value {
                Some(VariableShapeValue::Scalar(value)) => {
                    output.push_str(": ");
                    output.push_str(value);
                    output.push('\n');
                }
                Some(VariableShapeValue::Dynamic { binding, contract }) => {
                    render_dynamic_shape(binding, contract, indent, output);
                }
                None => output.push_str(": unknown\n"),
            }
        } else {
            output.push_str(":\n");
            if let Some(VariableShapeValue::Scalar(value)) = &child.value {
                output.push_str(&"  ".repeat(indent + 1));
                output.push_str("value: ");
                output.push_str(value);
                output.push('\n');
            }
            render_variable_shape(child, indent + 1, output);
        }
    }
}

fn render_dynamic_shape(
    binding: &VariableBinding,
    contract: &DynamicInputContract,
    indent: usize,
    output: &mut String,
) {
    output.push(':');
    let mut annotations = Vec::new();
    if contract.kind == DynamicInputKind::Order {
        annotations.push("ordered array of one-field entries".to_string());
    }
    if binding.nullable {
        annotations.push("nullable".to_string());
    }
    if let Some(default) = &binding.default {
        annotations.push(format!("default {}", input_default_label(default)));
    }
    if !annotations.is_empty() {
        output.push_str(" # ");
        output.push_str(&annotations.join("; "));
    }
    output.push('\n');

    match contract.kind {
        DynamicInputKind::Predicate => {
            write_shape_line(output, indent + 1, "and", "[<predicate>]");
            write_shape_line(output, indent + 1, "or", "[<predicate>]");
            write_shape_line(output, indent + 1, "not", "<predicate>");
            for field in &contract.fields {
                output.push_str(&"  ".repeat(indent + 1));
                output.push_str(&field.key);
                output.push_str(":\n");
                for operator in &field.operators {
                    write_shape_line(
                        output,
                        indent + 2,
                        operator.as_str(),
                        &dynamic_operand_type(field.data_type.as_str(), *operator),
                    );
                }
            }
        }
        DynamicInputKind::Order => {
            for field in &contract.fields {
                let directions = format!(
                    "enum({})",
                    field
                        .directions
                        .iter()
                        .map(|direction| format!("\"{}\"", direction.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                write_shape_line(output, indent + 1, &field.key, &directions);
            }
        }
    }
}

fn dynamic_operand_type(data_type: &str, operator: DynamicPredicateOperator) -> String {
    match operator {
        DynamicPredicateOperator::In | DynamicPredicateOperator::NotIn => {
            format!("{data_type}[]")
        }
        DynamicPredicateOperator::IsNull => "boolean".to_string(),
        _ => data_type.to_string(),
    }
}

fn write_shape_line(output: &mut String, indent: usize, key: &str, value: &str) {
    output.push_str(&"  ".repeat(indent));
    output.push_str(key);
    output.push_str(": ");
    output.push_str(value);
    output.push('\n');
}
