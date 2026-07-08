//! Lints: advisory diagnostics about queries that are valid but likely
//! slow. Gated on [`DiagnosticsDemand`] like the checks, and additionally
//! on a [`LintConfig`] singleton — no configuration, no lints.
//!
//! The unindexed-scan family flags plans PostgreSQL cannot serve from an
//! index: relation selections joining over unindexed foreign-key columns,
//! and multi-step predicate paths scanning or joining on unindexed
//! columns.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query, SystemExt, With};

use crate::catalog::{
    Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, ForeignKey, TableId, TableRef,
    TableResolution,
};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::DefDecl;
use crate::entities::expression::{Expr, PathAnchor, PathSegment};
use crate::entities::field_selection::{SelectionTree, TreeViews};
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand, NodeKey,
    Severity, Span, emit_diagnostic,
};

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
    // Views lowered facts ambiently: behind the Complete barrier.
    bowl.add_system(lint_definitions.run_during(bowl::Phase::Complete))
        .await;
}

/// Walks each definition's resolved selection tree, flagging unindexed
/// join and scan columns per the configured severity.
async fn lint_definitions(
    _: Query<Entity, With<DiagnosticsDemand>>,
    config: Query<(Entity, &LintConfig)>,
    defs: Query<(Entity, &DefDecl, &NodeKey, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (config_entity, config) = config.item();
    let Some(severity) = config.unindexed_scan_severity else {
        return;
    };
    let (def_entity, _decl, def_key, file) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();

    let tree = SelectionTree::collect(&views);
    let mut walk = LintWalk {
        catalog: snapshot.catalog(),
        tree: &tree,
        findings: Vec::new(),
    };

    // Fragment bodies lint against their declared target; query roots
    // against the table each names.
    if let Some((_, _, target, _, _)) = tree
        .fragments
        .iter()
        .find(|(entity, _, _, _, _)| *entity == def_entity)
    {
        if let Some(table) = walk.catalog.table_ref_for(TableRef::parse(&target.name)) {
            walk.lint_selection_set(table.id, *def_key);
        }
    } else {
        let roots: Vec<_> = tree
            .fields_under(*def_key)
            .map(|(_, field, key, _)| (*field, *key))
            .collect();
        for (field, key) in roots {
            if let TableResolution::Found(table) = walk
                .catalog
                .resolve_table_ref_for(TableRef::parse(&field.name))
            {
                let table_id = table.id;
                walk.lint_clauses(table_id, key);
                walk.lint_selection_set(table_id, key);
            }
        }
    }

    for (span, code, message) in walk.findings {
        emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([def_entity, catalog_entity, config_entity]),
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

struct LintWalk<'a> {
    catalog: &'a Catalog,
    tree: &'a SelectionTree<'a>,
    findings: Vec<(Span, DiagnosticCode, String)>,
}

impl LintWalk<'_> {
    /// Relation selections in one set: their join columns, their clauses
    /// against the relation's table, then their nested sets.
    fn lint_selection_set(&mut self, table: TableId, parent: NodeKey) {
        let fields: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(_, field, key, _)| (*field, *key))
            .collect();
        for (field, key) in fields {
            let reference = FieldRef {
                target: TableRef::parse(&field.name),
                selector: field.relation_path.as_deref(),
            };
            let FieldCheckResult::Relation(relation) = self.catalog.check_field_ref(table, reference)
            else {
                continue;
            };
            let relation_table = relation.table.id;
            self.lint_foreign_key(
                relation.foreign_key,
                field.name_span,
                DiagnosticCode::UnindexedJoinColumn,
                &format!(
                    "relation `{}` joins on unindexed column",
                    reference.display_text()
                ),
                "this can be slow",
            );
            self.lint_clauses(relation_table, key);
            self.lint_selection_set(relation_table, key);
        }
    }

    fn lint_clauses(&mut self, table: TableId, parent: NodeKey) {
        for (_, clause, _, _) in self.tree.clauses_under(parent) {
            if let ClauseFact::Where { expr } = clause {
                self.lint_predicate(table, expr);
            }
        }
    }

    fn lint_predicate(&mut self, table: TableId, expr: &Expr) {
        match expr {
            Expr::Binary { lhs, rhs, .. } => {
                self.lint_predicate(table, lhs);
                self.lint_predicate(table, rhs);
            }
            Expr::Path {
                anchor: PathAnchor::Current,
                segments,
                ..
            } if segments.len() >= 2 => self.lint_predicate_path(table, segments),
            Expr::Path { .. } | Expr::Literal { .. } | Expr::Variable { .. } | Expr::Error { .. } => {}
        }
    }

    /// Multi-step predicate paths become nested scans: every relation step
    /// should join over indexed columns and the terminal column should be
    /// indexed itself.
    fn lint_predicate_path(&mut self, table: TableId, segments: &[PathSegment]) {
        let Some((last, relations)) = segments.split_last() else {
            return;
        };
        let mut current = table;
        for segment in relations {
            let reference = FieldRef {
                target: TableRef::parse(&segment.name),
                selector: segment.relation_path.as_deref(),
            };
            let FieldCheckResult::Relation(relation) =
                self.catalog.check_field_ref(current, reference)
            else {
                return;
            };
            self.lint_foreign_key(
                relation.foreign_key,
                segment.span,
                DiagnosticCode::UnindexedPredicateJoinColumn,
                &format!(
                    "predicate relation `{}` joins on unindexed column",
                    reference.display_text()
                ),
                "nested scans can be slow",
            );
            current = relation.table.id;
        }

        let reference = FieldRef {
            target: TableRef::parse(&last.name),
            selector: last.relation_path.as_deref(),
        };
        let FieldCheckResult::Column(column) = self.catalog.check_field_ref(current, reference)
        else {
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
        self.findings.push((
            last.span,
            DiagnosticCode::UnindexedScanColumn,
            format!(
                "predicate path `{path_label}` filters on unindexed column `{}`; nested scans can be slow",
                self.column_label(column.id)
            ),
        ));
    }

    /// One finding per unindexed column on either side of the join.
    fn lint_foreign_key(
        &mut self,
        foreign_key: &ForeignKey,
        span: Span,
        code: DiagnosticCode,
        prefix: &str,
        suffix: &str,
    ) {
        for column_id in foreign_key
            .from_columns
            .iter()
            .chain(foreign_key.to_columns.iter())
        {
            let Some(column) = self.catalog.column_by_id(*column_id) else {
                continue;
            };
            if column.is_indexed {
                continue;
            }
            self.findings.push((
                span,
                code,
                format!("{prefix} `{}`; {suffix}", self.column_label(column.id)),
            ));
        }
    }

    fn column_label(&self, column: crate::catalog::ColumnId) -> String {
        let Some(column) = self.catalog.column_by_id(column) else {
            return String::new();
        };
        self.catalog.table_by_id(column.table).map_or_else(
            || column.name.clone(),
            |table| format!("{}.{}", table.name, column.name),
        )
    }
}
