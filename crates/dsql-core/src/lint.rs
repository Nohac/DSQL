//! Lints: advisory diagnostics about queries that are valid but likely
//! slow. Gated on [`DiagnosticsDemand`] like the checks, and additionally
//! on a [`LintConfig`] singleton — no configuration, no lints.
//!
//! The unindexed-scan family flags plans PostgreSQL cannot serve from an
//! index: relation selections joining over unindexed foreign-key columns,
//! and multi-step predicate paths scanning or joining on unindexed
//! columns. Both lints are tracked per resolution fact — the resolver
//! already established each name's meaning (including every predicate
//! path), so no walk and no ambient reads happen here.

use crate::schema::dsql_schema;
use bowl::{Commands, Component, DerivedFrom, Entity, Query, Registrar, With};

use crate::catalog::{Catalog, CatalogSnapshot, Relation};
use crate::entities::expression::PathAnchor;
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, Severity,
    Span, emit_diagnostic,
};
use crate::resolution::{PathTerminal, ResolvedClause, ResolvedSelection, SelectionTarget};

/// Lint configuration, one singleton per bowl. Project loading inserts it
/// from `[lint]` in `dsql.toml`; a bowl without one lints nothing.
///
/// Fingerprinted so a config change retires and re-derives every lint.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct LintConfig {
    /// Severity of the unindexed-scan lints; `None` turns them off.
    pub unindexed_scan_severity: Option<Severity>,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            unindexed_scan_severity: Some(Severity::Info),
        }
    }
}

pub fn register_lints(reg: &mut Registrar<'_>) {
    reg.system(lint_relations);
    reg.system(lint_predicates);
}

/// Flags relation selections joining over unindexed foreign-key columns:
/// one tracked invocation per resolution fact.
async fn lint_relations(
    _: Query<Entity, With<DiagnosticsDemand>>,
    config: Query<(Entity, &LintConfig)>,
    resolutions: Query<(Entity, &ResolvedSelection, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (config_entity, config) = config.item();
    let Some(severity) = config.unindexed_scan_severity else {
        return;
    };
    let (resolution_entity, resolved, file) = resolutions.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let SelectionTarget::Relation { relation, .. } = &resolved.target else {
        return;
    };
    let Some(relation) = catalog.relation_by_id(*relation) else {
        return;
    };

    for finding in unindexed_join_columns(catalog, relation) {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([resolution_entity, catalog_entity, config_entity]),
                file: file.0,
                span: resolved.name_span,
                severity,
                source: DiagnosticSource::Lint,
                code: DiagnosticCode::UnindexedJoinColumn,
                message: format!(
                    "relation `{}` joins on unindexed column `{finding}`; this can be slow",
                    resolved.written
                ),
            },
        );
    }
}

/// Flags multi-step predicate paths that scan or join over unindexed
/// columns: one tracked invocation per clause resolution, read straight
/// from the resolved paths.
async fn lint_predicates(
    _: Query<Entity, With<DiagnosticsDemand>>,
    config: Query<(Entity, &LintConfig)>,
    resolutions: Query<(Entity, &ResolvedClause, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (config_entity, config) = config.item();
    let Some(severity) = config.unindexed_scan_severity else {
        return;
    };
    let (resolution_entity, resolved, file) = resolutions.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let mut findings: Vec<Finding> = Vec::new();
    for path in &resolved.paths {
        // Only current-anchored, relation-stepping paths become nested
        // scans; single-segment paths filter the clause's own table, and
        // root-anchored paths keep the pre-resolution behavior of not
        // linting (their scan shape differs and needs its own rule).
        if path.anchor != PathAnchor::Current || path.relations.is_empty() {
            continue;
        }
        for step in &path.relations {
            let Some(relation) = catalog.relation_by_id(step.relation) else {
                continue;
            };
            for finding in unindexed_join_columns(catalog, relation) {
                findings.push((
                    step.span,
                    DiagnosticCode::UnindexedPredicateJoinColumn,
                    format!(
                        "predicate relation `{}` joins on unindexed column `{finding}`; nested scans can be slow",
                        step.display
                    ),
                ));
            }
        }
        let PathTerminal::Column {
            span,
            display,
            column,
            ..
        } = &path.terminal
        else {
            continue;
        };
        let Some(column_info) = catalog.column_by_id(*column) else {
            continue;
        };
        if column_info.is_indexed {
            continue;
        }
        let path_label = format!(
            ".{}",
            path.relations
                .iter()
                .map(|step| step.display.clone())
                .chain(std::iter::once(display.clone()))
                .collect::<Vec<_>>()
                .join(".")
        );
        findings.push((
            *span,
            DiagnosticCode::UnindexedScanColumn,
            format!(
                "predicate path `{path_label}` filters on unindexed column `{}`; nested scans can be slow",
                column_label(catalog, *column)
            ),
        ));
    }
    for (span, code, message) in findings {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([resolution_entity, catalog_entity, config_entity]),
                file: file.0,
                span,
                severity,
                source: DiagnosticSource::Lint,
                code,
                message,
            },
        );
    }
}

type Finding = (Span, DiagnosticCode, String);

/// The unindexed columns on either side of an effective relation join.
fn unindexed_join_columns(catalog: &Catalog, relation: &Relation) -> Vec<String> {
    relation
        .local_columns
        .iter()
        .chain(relation.target_columns.iter())
        .filter_map(|column_id| {
            let column = catalog.column_by_id(*column_id)?;
            (!column.is_indexed).then(|| column_label(catalog, column.id))
        })
        .collect()
}

fn column_label(catalog: &Catalog, column: crate::catalog::ColumnId) -> String {
    let Some(column) = catalog.column_by_id(column) else {
        return String::new();
    };
    catalog.table_by_id(column.table).map_or_else(
        || column.name.clone(),
        |table| format!("{}.{}", table.name, column.name),
    )
}
