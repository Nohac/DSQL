//! Lints: advisory diagnostics about queries that are valid but likely
//! slow. Gated on [`DiagnosticsDemand`] like the checks, and additionally
//! on a [`LintConfig`] singleton — no configuration, no lints.
//!
//! The unindexed-scan family flags plans PostgreSQL cannot serve from an
//! index: relation selections joining over unindexed foreign-key columns,
//! and multi-step predicate paths scanning or joining on unindexed
//! columns. Both lints are tracked per resolution fact — the resolver
//! already established each name's meaning, so no walk happens here and
//! no phase barrier is needed.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query, With};

use crate::catalog::{
    Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, ForeignKey, TableId, TableRef,
};
use crate::entities::clause::ClauseFact;
use crate::entities::expression::{Expr, PathAnchor, PathSegment};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, Severity,
    Span, emit_diagnostic,
};
use crate::resolution::{ResolvedClause, ResolvedSelection, SelectionTarget};

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

pub async fn register_lints(bowl: &Bowl) {
    bowl.add_system(lint_relations).await;
    bowl.add_system(lint_predicates).await;
}

/// Flags relation selections joining over unindexed foreign-key columns:
/// one tracked invocation per resolution fact.
async fn lint_relations(
    _: Query<Entity, With<DiagnosticsDemand>>,
    config: Query<(Entity, &LintConfig)>,
    resolutions: Query<(Entity, &ResolvedSelection, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let (config_entity, config) = config.item();
    let Some(severity) = config.unindexed_scan_severity else {
        return;
    };
    let (resolution_entity, resolved, file) = resolutions.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let SelectionTarget::Relation { foreign_key, .. } = &resolved.target else {
        return;
    };
    let Some(foreign_key) = catalog.foreign_key_by_id(*foreign_key) else {
        return;
    };

    for finding in unindexed_join_columns(catalog, foreign_key) {
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
/// columns: one tracked invocation per clause resolution.
async fn lint_predicates(
    _: Query<Entity, With<DiagnosticsDemand>>,
    config: Query<(Entity, &LintConfig)>,
    resolutions: Query<(Entity, &ResolvedClause, &BelongsToFile)>,
    clauses: bowl::View<'_, (Entity, &ClauseFact)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let (config_entity, config) = config.item();
    let Some(severity) = config.unindexed_scan_severity else {
        return;
    };
    let (resolution_entity, resolved, file) = resolutions.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let Some(context) = resolved.context else {
        return;
    };
    // The clause fact is Evaluate-lowered and referenced by entity id off
    // the tracked resolution row; the ambient lookup can never observe a
    // partially-derived generation because the resolution derives strictly
    // after the clause it points at.
    let Some((_, ClauseFact::Where { expr })) = clauses
        .iter()
        .find(|(entity, _)| *entity == resolved.clause)
    else {
        return;
    };

    let mut findings = Vec::new();
    collect_predicate_findings(catalog, context.table, expr, &mut findings);
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

fn collect_predicate_findings(
    catalog: &Catalog,
    table: TableId,
    expr: &Expr,
    findings: &mut Vec<Finding>,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            collect_predicate_findings(catalog, table, lhs, findings);
            collect_predicate_findings(catalog, table, rhs, findings);
        }
        Expr::Path {
            anchor: PathAnchor::Current,
            segments,
            ..
        } if segments.len() >= 2 => {
            collect_predicate_path_findings(catalog, table, segments, findings);
        }
        Expr::Path { .. } | Expr::Literal { .. } | Expr::Variable { .. } | Expr::Error { .. } => {}
    }
}

/// Multi-step predicate paths become nested scans: every relation step
/// should join over indexed columns and the terminal column should be
/// indexed itself.
fn collect_predicate_path_findings(
    catalog: &Catalog,
    table: TableId,
    segments: &[PathSegment],
    findings: &mut Vec<Finding>,
) {
    let Some((last, relations)) = segments.split_last() else {
        return;
    };
    let mut current = table;
    for segment in relations {
        let reference = FieldRef {
            target: TableRef::parse(&segment.name),
            selector: segment.relation_path.as_deref(),
        };
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(current, reference)
        else {
            return;
        };
        for finding in unindexed_join_columns(catalog, relation.foreign_key) {
            findings.push((
                segment.span,
                DiagnosticCode::UnindexedPredicateJoinColumn,
                format!(
                    "predicate relation `{}` joins on unindexed column `{finding}`; nested scans can be slow",
                    reference.display_text()
                ),
            ));
        }
        current = relation.table.id;
    }

    let reference = FieldRef {
        target: TableRef::parse(&last.name),
        selector: last.relation_path.as_deref(),
    };
    let FieldCheckResult::Column(column) = catalog.check_field_ref(current, reference) else {
        return;
    };
    if column.is_indexed {
        return;
    }
    let path_label = format!(
        ".{}",
        segments
            .iter()
            .map(|segment| {
                FieldRef {
                    target: TableRef::parse(&segment.name),
                    selector: segment.relation_path.as_deref(),
                }
                .display_text()
            })
            .collect::<Vec<_>>()
            .join(".")
    );
    findings.push((
        last.span,
        DiagnosticCode::UnindexedScanColumn,
        format!(
            "predicate path `{path_label}` filters on unindexed column `{}`; nested scans can be slow",
            column_label(catalog, column.id)
        ),
    ));
}

/// The unindexed columns on either side of a foreign-key join, labelled.
fn unindexed_join_columns(catalog: &Catalog, foreign_key: &ForeignKey) -> Vec<String> {
    foreign_key
        .from_columns
        .iter()
        .chain(foreign_key.to_columns.iter())
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
