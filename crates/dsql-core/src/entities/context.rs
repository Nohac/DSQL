//! Explicit trusted-context declarations and their scope-aware resolution.
//!
//! [`ContextIndex`] is the only source of trusted-context types. Variable
//! inference, policy compilation, planning, and editor services consume its
//! resolved entries instead of deriving contracts from use sites.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Phase, Query, Registrar, SystemExt,
    View, Where, With,
};

use crate::catalog::{Catalog, CatalogSnapshot, CatalogTypeShape, DataType, TypeKey, WireEncoding};
use crate::entities::definition::DefIndex;
use crate::entities::document::ParsedFile;
use crate::entities::expression::Sigil;
use crate::entities::variable::VariableUse;
use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::schema::{AstFacts, dsql_schema};
use crate::service::completion::{
    CompletionContext, CompletionItem, CompletionKind, CompletionRequest, CompletionSite,
    emit_completion_candidate,
};
use crate::service::definition::{DefinitionRequest, DefinitionTarget};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::source::{BelongsToHost, FilePath, ResolutionScope, ScopeImports};

/// One entry lowered from a scope-level `context` declaration block.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ContextDecl {
    /// Trusted-context key without the generated `context.` prefix.
    pub name: String,
    /// Precise source span of [`ContextDecl::name`].
    pub name_span: Span,
    /// Optional provider schema. Built-in logical types remain unqualified.
    pub type_schema: Option<String>,
    /// Built-in logical name or provider-internal type name.
    pub type_name: String,
    /// Precise source span of the complete type, including collection suffix.
    pub type_span: Span,
    /// Whether the declaration accepts a collection of the named scalar type.
    pub collection: bool,
}

/// Authoritative value contract resolved from one [`ContextDecl`].
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ContextValueContract {
    pub data_type: DataType,
    pub wire: WireEncoding,
    pub provider_type: Option<TypeKey>,
    pub collection: bool,
    pub closed_values: Vec<String>,
}

/// Stable type-resolution failure attached to one indexed declaration.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ContextTypeProblem {
    UnknownBuiltin { name: String },
    UnknownProvider { key: TypeKey },
    ProviderArray { key: TypeKey },
    UnsupportedWire { name: String },
}

/// One context entry after catalog resolution, including its source target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextEntry {
    pub declaration: Entity,
    pub file: Entity,
    pub file_path: String,
    pub scope: String,
    pub name: String,
    pub name_span: Span,
    pub type_span: Span,
    pub contract: Option<ContextValueContract>,
    pub problem: Option<ContextTypeProblem>,
    pub embedded: bool,
}

/// Tracked, scope-aware index of every explicit trusted-context declaration.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct ContextIndex {
    pub entries: Vec<ContextEntry>,
}

impl Hash for ContextIndex {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for entry in &self.entries {
            // Entity ids are navigation payload, not semantic identity. Stable
            // paths and spans keep edits honest without churning on re-lowering.
            entry.file_path.hash(state);
            entry.scope.hash(state);
            entry.name.hash(state);
            entry.name_span.hash(state);
            entry.type_span.hash(state);
            entry.contract.hash(state);
            entry.problem.hash(state);
            entry.embedded.hash(state);
        }
    }
}

/// Result of resolving one context name in an effective scope.
pub(crate) enum ContextLookup<'a> {
    Resolved(&'a ContextEntry, &'a ContextValueContract),
    Unknown,
    Ambiguous(Vec<&'a ContextEntry>),
    Invalid,
}

impl ContextIndex {
    /// All declarations visible from `scope` with exactly `name`.
    pub fn visible<'a>(
        &'a self,
        scope: &str,
        name: &str,
        imports: &'a ScopeImports,
    ) -> Vec<&'a ContextEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.name == name
                    && imports
                        .visible_from(scope)
                        .any(|visible| visible == entry.scope)
            })
            .collect()
    }

    /// Resolves one name only when the effective scope supplies one valid entry.
    pub(crate) fn lookup<'a>(
        &'a self,
        scope: &str,
        name: &str,
        imports: &'a ScopeImports,
    ) -> ContextLookup<'a> {
        let visible = self.visible(scope, name, imports);
        match visible.as_slice() {
            [] => ContextLookup::Unknown,
            [entry] => entry
                .contract
                .as_ref()
                .map_or(ContextLookup::Invalid, |contract| {
                    ContextLookup::Resolved(entry, contract)
                }),
            _ => ContextLookup::Ambiguous(visible),
        }
    }
}

/// Source navigation and declaration contract for one context occurrence.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct ResolvedContextUse {
    pub name: String,
    pub span: Span,
    pub resolution: ContextUseResolution,
}

/// Scope lookup outcome for one [`ResolvedContextUse`].
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ContextUseResolution {
    Resolved {
        file: Entity,
        name_span: Span,
        contract: ContextValueContract,
    },
    Unknown,
    Ambiguous {
        providers: Vec<String>,
    },
    Invalid,
}

/// Validates one use-site expectation without replacing declaration semantics.
pub(crate) fn validate_context_use(
    name: &str,
    declared: &ContextValueContract,
    expected: &ContextValueContract,
) -> Result<Vec<String>, String> {
    let type_matches =
        if declared.wire == WireEncoding::TextCast || expected.wire == WireEncoding::TextCast {
            declared.wire == expected.wire && declared.provider_type == expected.provider_type
        } else {
            declared.data_type == expected.data_type && declared.wire == expected.wire
        };
    if !type_matches || declared.collection != expected.collection {
        return Err(format!(
            "trusted context `context.{name}` is declared as `{}` but this use requires `{}`",
            context_type_label(declared),
            context_type_label(expected),
        ));
    }

    if expected.closed_values.is_empty() {
        return Ok(declared.closed_values.clone());
    }
    if declared.closed_values.is_empty()
        || expected
            .closed_values
            .iter()
            .all(|value| declared.closed_values.contains(value))
    {
        return Ok(expected.closed_values.clone());
    }
    Err(format!(
        "trusted context `context.{name}` does not allow every value required by this use"
    ))
}

/// Human-facing logical/provider label for one resolved context contract.
pub(crate) fn context_type_label(contract: &ContextValueContract) -> String {
    let base = contract.provider_type.as_ref().map_or_else(
        || contract.data_type.as_str().to_string(),
        |key| format!("{}::{}", key.schema, key.name),
    );
    if contract.collection {
        format!("{base}[]")
    } else {
        base
    }
}

/// Owns `context_def` and all declaration-driven context semantics.
pub struct Context;

impl LanguageEntity for Context {
    const NAME: &'static str = "context";

    fn register(registrar: &mut Registrar<'_>) {
        registrar.system(index_contexts.run_during(Phase::Complete));
        registrar.system(resolve_context_uses);
        registrar.system(check_context_declarations);
        registrar.system(check_context_uses);
        registrar.system(hover_context_declarations);
        registrar.system(hover_context_uses);
        registrar.system(complete_context_uses.run_during(Phase::Complete));
        registrar.system(define_context_uses);
    }
}

impl LowerStage for Context {
    fn lower(
        context: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        for entry in context
            .cst
            .children(node)
            .filter(|child| context.cst.match_rule(*child, Rule::ContextEntry))
        {
            let Some(name_span) = direct_name(context.cst, entry) else {
                continue;
            };
            let Some(context_type) = direct_rule(context.cst, entry, Rule::ContextType) else {
                continue;
            };
            let Some(qualified) = direct_rule(context.cst, context_type, Rule::QualifiedName)
            else {
                continue;
            };
            let parts = crate::entities::direct_names(context.cst, qualified);
            let (type_schema, type_name) = match parts.as_slice() {
                [name] => (None, text(context.source, *name).to_string()),
                [schema, name] => (
                    Some(text(context.source, *schema).to_string()),
                    text(context.source, *name).to_string(),
                ),
                _ => continue,
            };
            let declaration = ContextDecl {
                name: text(context.source, name_span).to_string(),
                name_span,
                type_schema,
                type_name,
                type_span: node_span(context.cst, context_type),
                collection: context
                    .cst
                    .children(context_type)
                    .any(|child| context.cst.match_token(child, Token::LBracket).is_some()),
            };
            commands.insert((
                DerivedFrom::new(context.file),
                BelongsToFile(context.file),
                NodeKey {
                    file: context.file,
                    node: entry.0,
                },
                ResolutionScope(context.scope.to_string()),
                declaration,
            ));
        }
        None
    }
}

impl FormatStage for Context {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.context_definition(node);
    }
}

async fn index_contexts(
    _: Query<(Entity, &ParsedFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    definitions: Query<(Entity, &DefIndex)>,
    declarations: View<'_, (Entity, &ContextDecl, &BelongsToFile, &ResolutionScope)>,
    files: View<'_, (Entity, &FilePath)>,
    embedded: View<'_, (Entity, &BelongsToHost)>,
    mut commands: Commands<(dsql_schema::DefIndex,)>,
) {
    let (_, snapshot) = catalog.item();
    let file_paths = files
        .iter()
        .map(|(entity, path)| (entity, path.0.as_str()))
        .collect::<BTreeMap<_, _>>();
    let embedded_files = embedded
        .iter()
        .map(|(entity, _)| entity)
        .collect::<BTreeSet<_>>();
    let mut entries = declarations
        .iter()
        .map(|(entity, declaration, file, scope)| {
            let (contract, problem) = resolve_contract(snapshot.catalog(), declaration);
            ContextEntry {
                declaration: entity,
                file: file.0,
                file_path: file_paths
                    .get(&file.0)
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                scope: scope.0.clone(),
                name: declaration.name.clone(),
                name_span: declaration.name_span,
                type_span: declaration.type_span,
                contract,
                problem,
                embedded: embedded_files.contains(&file.0),
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.scope
            .cmp(&right.scope)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.name_span.start.cmp(&right.name_span.start))
    });
    commands
        .entity(definitions.item().0)
        .insert(ContextIndex { entries });
}

fn resolve_contract(
    catalog: &Catalog,
    declaration: &ContextDecl,
) -> (Option<ContextValueContract>, Option<ContextTypeProblem>) {
    if let Some(schema) = &declaration.type_schema {
        let key = TypeKey::new(schema, &declaration.type_name);
        let Some(data_type) = catalog.type_by_key(&key) else {
            return (None, Some(ContextTypeProblem::UnknownProvider { key }));
        };
        if matches!(data_type.shape, CatalogTypeShape::Array { .. }) {
            return (None, Some(ContextTypeProblem::ProviderArray { key }));
        }
        if data_type.capabilities.wire == WireEncoding::Unsupported {
            return (
                None,
                Some(ContextTypeProblem::UnsupportedWire {
                    name: format!("{}::{}", key.schema, key.name),
                }),
            );
        }
        let closed_values =
            catalog
                .enum_type_for_type(data_type.id)
                .map_or_else(Vec::new, |(_, enumeration)| {
                    enumeration
                        .variants
                        .iter()
                        .map(|variant| variant.variant.clone())
                        .collect()
                });
        return (
            Some(ContextValueContract {
                data_type: data_type.logical_data_type(),
                wire: data_type.capabilities.wire,
                provider_type: Some(key),
                collection: declaration.collection,
                closed_values,
            }),
            None,
        );
    }

    let Some(data_type) = Catalog::resolve_logical_type_name(&declaration.type_name) else {
        return (
            None,
            Some(ContextTypeProblem::UnknownBuiltin {
                name: declaration.type_name.clone(),
            }),
        );
    };
    let capabilities = Catalog::builtin_capabilities(data_type);
    if capabilities.wire == WireEncoding::Unsupported {
        return (
            None,
            Some(ContextTypeProblem::UnsupportedWire {
                name: declaration.type_name.clone(),
            }),
        );
    }
    (
        Some(ContextValueContract {
            data_type,
            wire: capabilities.wire,
            provider_type: None,
            collection: declaration.collection,
            closed_values: Vec::new(),
        }),
        None,
    )
}

async fn resolve_context_uses(
    uses: Query<(Entity, &VariableUse, &BelongsToFile, &ResolutionScope)>,
    index: Query<(Entity, &ContextIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::ResolvedContextUse,)>,
) {
    let (use_entity, variable, file, scope) = uses.item();
    if variable.sigil() != Sigil::Context {
        return;
    }
    let Some(name) = variable.0.name.as_deref() else {
        return;
    };
    let (index_entity, index) = index.item();
    let (_, imports) = imports.item();
    let resolution = match index.lookup(&scope.0, name, imports) {
        ContextLookup::Resolved(entry, contract) => ContextUseResolution::Resolved {
            file: entry.file,
            name_span: entry.name_span,
            contract: contract.clone(),
        },
        ContextLookup::Unknown => ContextUseResolution::Unknown,
        ContextLookup::Ambiguous(entries) => {
            let providers = entries
                .iter()
                .map(|entry| entry.scope.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            ContextUseResolution::Ambiguous { providers }
        }
        ContextLookup::Invalid => ContextUseResolution::Invalid,
    };
    commands.insert((
        DerivedFrom::many([use_entity, index_entity]),
        BelongsToFile(file.0),
        ResolvedContextUse {
            name: name.to_string(),
            span: variable.0.span,
            resolution,
        },
    ));
}

async fn check_context_declarations(
    _: Query<Entity, With<DiagnosticsDemand>>,
    index: Query<(Entity, &ContextIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (index_entity, index) = index.item();
    let (_, imports) = imports.item();

    for entry in &index.entries {
        if entry.embedded {
            emit_context_diagnostic(
                &mut commands,
                index_entity,
                entry,
                entry.name_span,
                DiagnosticCode::InvalidContextDefinition,
                "context declarations must be standalone DSQL definitions".to_string(),
            );
        }
        if let Some(problem) = &entry.problem {
            emit_context_diagnostic(
                &mut commands,
                index_entity,
                entry,
                entry.type_span,
                DiagnosticCode::InvalidContextDefinition,
                context_problem_message(problem),
            );
        }
    }

    let mut local_groups = BTreeMap::<(&str, &str), Vec<&ContextEntry>>::new();
    for entry in &index.entries {
        local_groups
            .entry((entry.scope.as_str(), entry.name.as_str()))
            .or_default()
            .push(entry);
    }
    for ((_, name), entries) in local_groups {
        for duplicate in entries.iter().skip(1) {
            emit_context_diagnostic(
                &mut commands,
                index_entity,
                duplicate,
                duplicate.name_span,
                DiagnosticCode::DuplicateDefinition,
                format!("duplicate context entry `{name}`"),
            );
        }
    }

    for local in &index.entries {
        if let Some(imported) = index.entries.iter().find(|candidate| {
            candidate.name == local.name
                && imports
                    .imports_of(&local.scope)
                    .any(|provider| provider == candidate.scope)
        }) {
            emit_context_diagnostic(
                &mut commands,
                index_entity,
                local,
                local.name_span,
                DiagnosticCode::DuplicateDefinition,
                format!(
                    "context entry `{}` collides with a declaration imported from scope `{}`",
                    local.name, imported.scope
                ),
            );
        }
    }

    for consumer in imports.0.keys() {
        let mut groups = BTreeMap::<&str, Vec<&ContextEntry>>::new();
        for entry in &index.entries {
            if imports
                .imports_of(consumer)
                .any(|provider| provider == entry.scope)
            {
                groups.entry(&entry.name).or_default().push(entry);
            }
        }
        for (name, entries) in groups {
            let providers = entries
                .iter()
                .map(|entry| entry.scope.as_str())
                .collect::<BTreeSet<_>>();
            if providers.len() < 2 {
                continue;
            }
            let first = entries[0];
            emit_context_diagnostic(
                &mut commands,
                index_entity,
                first,
                first.name_span,
                DiagnosticCode::AmbiguousTrustedContext,
                format!(
                    "context entry `{name}` is provided to scope `{consumer}` by scopes `{}`",
                    providers.into_iter().collect::<Vec<_>>().join("`, `")
                ),
            );
        }
    }
}

fn context_problem_message(problem: &ContextTypeProblem) -> String {
    match problem {
        ContextTypeProblem::UnknownBuiltin { name } => format!(
            "unknown built-in context type `{name}`; catalog/provider types must be schema-qualified"
        ),
        ContextTypeProblem::UnknownProvider { key } => {
            format!("provider type `{}::{}` not found", key.schema, key.name)
        }
        ContextTypeProblem::ProviderArray { key } => format!(
            "provider array type `{}::{}` cannot be declared directly; declare its element type with `[]`",
            key.schema, key.name
        ),
        ContextTypeProblem::UnsupportedWire { name } => {
            format!("context type `{name}` has no supported input wire encoding")
        }
    }
}

fn emit_context_diagnostic(
    commands: &mut Commands<(dsql_schema::Diagnostic,)>,
    index: Entity,
    entry: &ContextEntry,
    span: Span,
    code: DiagnosticCode,
    message: String,
) {
    emit_diagnostic(
        commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(index),
            file: entry.file,
            span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code,
            message,
        },
    );
}

async fn check_context_uses(
    _: Query<Entity, With<DiagnosticsDemand>>,
    uses: Query<(Entity, &ResolvedContextUse, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, resolved, file) = uses.item();
    let (code, message) = match &resolved.resolution {
        ContextUseResolution::Unknown => (
            DiagnosticCode::UnknownTrustedContext,
            format!("trusted context `{}` is not declared", resolved.name),
        ),
        ContextUseResolution::Ambiguous { providers } => (
            DiagnosticCode::AmbiguousTrustedContext,
            format!(
                "trusted context `{}` is ambiguous across scopes `{}`",
                resolved.name,
                providers.join("`, `")
            ),
        ),
        ContextUseResolution::Resolved { .. } | ContextUseResolution::Invalid => return,
    };
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(entity),
            file: file.0,
            span: resolved.span,
            severity: Severity::Error,
            source: DiagnosticSource::Check,
            code,
            message,
        },
    );
}

async fn hover_context_declarations(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    declaration: Query<(Entity, &ContextDecl), Where<BowlEq<BelongsToFile>>>,
    index: Query<(Entity, &ContextIndex)>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = request.item();
    let (entity, declaration) = declaration.item();
    if !declaration.name_span.contains(cursor.0) && !declaration.type_span.contains(cursor.0) {
        return;
    }
    let (_, index) = index.item();
    let Some(entry) = index
        .entries
        .iter()
        .find(|entry| entry.declaration == entity)
    else {
        return;
    };
    let Some(contract) = &entry.contract else {
        return;
    };
    emit_hover_candidate(
        &mut commands,
        request,
        priority::VARIABLE,
        format!(
            "`{}` — `context.{}`: {} (trusted context, required)",
            declaration.name,
            declaration.name,
            context_type_label(contract)
        ),
    );
}

async fn hover_context_uses(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    uses: Query<(Entity, &ResolvedContextUse), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, resolved) = uses.item();
    if !resolved.span.contains(cursor.0) {
        return;
    }
    let ContextUseResolution::Resolved { contract, .. } = &resolved.resolution else {
        return;
    };
    emit_hover_candidate(
        &mut commands,
        request,
        priority::VARIABLE,
        format!(
            "`{}` — `context.{}`: {} (trusted context, required)",
            resolved.name,
            resolved.name,
            context_type_label(contract)
        ),
    );
}

async fn complete_context_uses(
    request: Query<(Entity, &CompletionContext), With<CompletionRequest>>,
    index: Query<(Entity, &ContextIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    let (request, context) = request.item();
    if context.site != CompletionSite::ContextVariable {
        return;
    }
    let (_, index) = index.item();
    let (_, imports) = imports.item();
    let mut by_name = BTreeMap::<&str, Vec<&ContextEntry>>::new();
    for entry in &index.entries {
        if imports
            .visible_from(&context.scope)
            .any(|visible| visible == entry.scope)
        {
            by_name.entry(&entry.name).or_default().push(entry);
        }
    }
    let items = by_name
        .into_iter()
        .filter_map(|(name, entries)| {
            let [entry] = entries.as_slice() else {
                return None;
            };
            let contract = entry.contract.as_ref()?;
            Some(CompletionItem {
                label: name.to_string(),
                kind: CompletionKind::Variable,
                detail: Some(format!(
                    "{} (trusted context, required)",
                    context_type_label(contract)
                )),
                documentation: None,
                insert_text: None,
            })
        })
        .collect();
    emit_completion_candidate(&mut commands, request, items);
}

async fn define_context_uses(
    request: Query<(Entity, &BelongsToFile, &Cursor), With<DefinitionRequest>>,
    uses: Query<(Entity, &ResolvedContextUse), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::DefinitionAnswer,)>,
) {
    let (request, _, cursor) = request.item();
    let (_, resolved) = uses.item();
    if !resolved.span.contains(cursor.0) {
        return;
    }
    let ContextUseResolution::Resolved {
        file, name_span, ..
    } = resolved.resolution
    else {
        return;
    };
    commands.entity(request).insert(DefinitionTarget::Source {
        file,
        span: name_span,
    });
}
