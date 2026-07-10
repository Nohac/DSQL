//! Definition entity: named top-level definitions (queries and fragments),
//! the definition index, and the duplicate-fragment check.
//!
//! Queries and fragments are one entity because they are structurally the
//! same concept — a named definition with a selection set — and every stage
//! treats them symmetrically except where [`DefKind`] branches.

use crate::schema::{AstFacts, dsql_schema};
use std::fmt;

use bowl::{
    Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Phase, Query, Registrar, Singleton,
    SystemExt, View, Where, With,
};

use crate::catalog::{CatalogSnapshot, TableRef, TableResolution};
use crate::entities::document::ParsedFile;
use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{
    CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage,
};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};
use crate::service::hover::{Cursor, HoverCandidate, HoverEnriched, RequestKey, priority};
use crate::source::{ResolutionScope, ScopeImports};

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
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct DefDecl {
    pub kind: DefKind,
    pub name: String,
    /// Span of the name token, for name-precision diagnostics.
    pub name_span: Span,
    /// Span of the whole definition.
    pub span: Span,
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
/// so idempotent reruns invalidate nothing.
#[derive(Component, Hash)]
#[component(hash)]
pub struct DefIndex(Vec<(String, DefKind, String)>);

/// Owns `query_def` and `fragment_def`.
pub struct Definition;

impl LanguageEntity for Definition {
    const NAME: &'static str = "definition";

    fn register(reg: &mut Registrar<'_>) {
        // Ambient readers of lowered facts sit behind the Complete phase
        // barrier (the engine's same-phase race flag enforces this);
        // check_fragment_targets reads only tracked inputs and needs none.
        reg.system(index_defs.run_during(Phase::Complete));
        reg.system(check_duplicate_fragments.run_during(Phase::Complete));
        reg.system(check_import_collisions.run_during(Phase::Complete));
        reg.system(check_fragment_targets);
    }
}

/// A fragment's `on` target must resolve to a catalog table; its body is
/// only checked once it does (see `field_selection::check_selections`).
async fn check_fragment_targets(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &DefDecl, &FragmentTarget, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (fragment, _, target, file) = query.item();
    let (catalog_entity, snapshot) = catalog.item();

    match snapshot
        .catalog()
        .resolve_table_ref_for(TableRef::parse(&target.name))
    {
        TableResolution::Found(_) => {}
        TableResolution::NotFound { reference } => {
            emit_diagnostic(
                &mut commands,
                DiagnosticFacts {
                    derived_from: DerivedFrom::many([fragment, catalog_entity]),
                    file: file.0,
                    span: target.span,
                    severity: Severity::Error,
                    source: DiagnosticSource::Check,
                    code: DiagnosticCode::TableNotFound,
                    message: format!("table `{reference}` not found"),
                },
            );
        }
        TableResolution::Ambiguous {
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
                    derived_from: DerivedFrom::many([fragment, catalog_entity]),
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
        let Some(name_span) = direct_token(ctx.cst, node, Token::Name) else {
            // Error recovery can leave a def without a name; the parse
            // diagnostics already cover it.
            return None;
        };

        let kind = if ctx.cst.match_rule(node, Rule::QueryDef) {
            DefKind::Query
        } else {
            DefKind::Fragment
        };

        let decl = DefDecl {
            kind,
            name: text(ctx.source, name_span).to_string(),
            name_span,
            span: node_span(ctx.cst, node),
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

/// Aggregates the definition set into the [`DefIndex`] singleton, driven
/// per parsed file so any text change recomputes it. Ungated: spread
/// resolution and planning consume it, not just diagnostics. Runs at
/// Complete, behind the phase barrier its ambient view of lowered
/// definitions needs; index-tracked consumers replan when it commits, and
/// since services arbitrate through tracked joins (no settle-phase
/// answering), the extra generation costs latency only, never answers.
async fn index_defs(
    query: Query<(Entity, &ParsedFile)>,
    defs: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    mut commands: Commands<(dsql_schema::DefIndex,)>,
) {
    let _ = query.item();

    let mut entries: Vec<(String, DefKind, String)> = defs
        .iter()
        .map(|(_, decl, scope)| (scope.0.clone(), decl.kind, decl.name.clone()))
        .collect();
    entries.sort();

    commands.insert((Singleton::<DefIndex>::new(), DefIndex(entries)));
}

/// Duplicate fragment names are ambiguous at spread-resolution time, so
/// they are errors — scoped per resolution scope (the same name in two
/// independent scopes is fine, per docs/spec/resolution-scopes.md).
/// Query names are entry points and not checked here.
///
/// The [`DefIndex`] query keeps this check honest: the `View` of other
/// definitions contributes no memo deps, so without a tracked input over the
/// definition *set*, a row would never rerun when an unrelated definition is
/// added or removed — a surviving duplicate could go unreported.
async fn check_duplicate_fragments(
    _: Query<Entity, With<DiagnosticsDemand>>,
    query: Query<(Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    _index: Query<(Entity, &DefIndex)>,
    defs: View<'_, (Entity, &DefDecl, &ResolutionScope)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, decl, file, scope) = query.item();

    if decl.kind != DefKind::Fragment {
        return;
    }

    let Some((previous, _, _)) = defs.iter().find(|(other, other_decl, other_scope)| {
        *other < entity
            && other_decl.kind == DefKind::Fragment
            && other_decl.name == decl.name
            && other_scope.0 == scope.0
    }) else {
        return;
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
            message: format!("duplicate fragment `{}`", decl.name),
        },
    );
}

/// A local fragment whose name is also provided by an imported scope is a
/// diagnostic at the local definition (docs/spec/resolution-scopes.md).
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

    if decl.kind != DefKind::Fragment {
        return;
    }

    let Some((imported, _, imported_scope)) = defs.iter().find(|(_, other_decl, other_scope)| {
        other_decl.kind == DefKind::Fragment
            && other_decl.name == decl.name
            && imports
                .imports_of(&scope.0)
                .any(|import| import == other_scope.0)
    }) else {
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
                "fragment `{}` collides with a definition imported from scope `{}`",
                decl.name, imported_scope.0
            ),
        },
    );
}

impl FormatStage for Definition {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        if formatter.rule(node) == Some(Rule::QueryDef) {
            formatter.write_str("query");
            if let Some(name) = formatter.direct_token_text(node, Token::Name) {
                formatter.write_str(" ");
                formatter.write_str(&name);
            }
            for directive in formatter.direct_rules(node, Rule::Directive) {
                formatter.format_child(directive);
            }
        } else {
            formatter.write_str("fragment");
            if let Some(name) = formatter.direct_token_text(node, Token::Name) {
                formatter.write_str(" ");
                formatter.write_str(&name);
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

impl HoverStage for Definition {
    fn register_hover(reg: &mut Registrar<'_>) {
        // Fully tracked (a per-file bound join, no views), so it needs no
        // phase barrier: replanning orders it after enrichment and the
        // lowered facts it joins.
        reg.system(hover_definitions);
    }
}

/// Answers hover on a definition name with its kind and target: one
/// invocation per (request, definition-in-file) pair via the
/// `BelongsToFile` join, the fragment target riding the definition row as
/// an optional part.
/// One definition row in the hovered file: the declaration with its
/// optional fragment target riding along.
type DefInFile<'a> = (Entity, &'a DefDecl, Option<&'a FragmentTarget>);

async fn hover_definitions(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    defs: Query<DefInFile<'_>, Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_def_entity, decl, target) = defs.item();

    if !(decl.name_span.start <= cursor.0 && cursor.0 < decl.name_span.end) {
        return;
    }

    let text = match (decl.kind, target) {
        (DefKind::Query, _) => format!("query `{}`", decl.name),
        (DefKind::Fragment, Some(target)) => {
            format!("fragment `{}` on `{}`", decl.name, target.name)
        }
        (DefKind::Fragment, None) => format!("fragment `{}`", decl.name),
    };

    commands.insert((
        DerivedFrom::new(request),
        RequestKey(request),
        HoverCandidate {
            priority: priority::DEFINITION,
            text,
        },
    ));
}

impl CompletionStage for Definition {
    /// Definition keywords come from the grammar layer.
    fn register_completions(_reg: &mut Registrar<'_>) {}
}
