//! Semantic tokens service: classifies every name span of a file for
//! editor highlighting.
//!
//! Unlike hover and completion there is no per-entity stage: a name's
//! class (table, relation, column) is decided by what its context
//! resolves to, so classification is one top-down walk mirroring the
//! check walk — splitting it per entity would fragment a single
//! traversal. Only names that actually resolve are classified; broken
//! references stay unstyled and the diagnostics point at them instead.
//!
//! External callers drive it request/response:
//! `bowl.insert((SemanticTokensRequest, FilePath(...))).await
//!     .bind().take::<SemanticTokens>()`.

use bowl::{
    Bowl, Commands, Component, Entity, Eq as BowlEq, Phase, Query, SystemExt, View, Where, With,
};

use crate::catalog::{Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, TableId, TableRef};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::DefDecl;
use crate::entities::expression::{Expr, PathAnchor};
use crate::entities::field_selection::{SelectionTree, TreeViews};
use crate::facts::{BelongsToFile, NodeKey, Span};
use crate::source::{FilePath, SourceText};

/// Marks an entity as a semantic-tokens request; pair with [`FilePath`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct SemanticTokensRequest;

/// What a classified span highlights as.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum SemanticTokenKind {
    Schema,
    Table,
    Relation,
    Column,
    Fragment,
    Alias,
}

/// One classified span.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct SemanticToken {
    pub span: Span,
    pub kind: SemanticTokenKind,
}

/// The answer: every classified span of the file, span-sorted. A request
/// for an unknown file answers with no tokens.
#[derive(Debug, Component, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct SemanticTokens(pub Vec<SemanticToken>);

pub(crate) async fn register_semantic_tokens_pipeline(bowl: &Bowl) {
    bowl.add_system(answer_semantic_tokens.run_during(Phase::Complete))
        .await;
}

/// The file side of the request outer join: matched per equal path, or
/// `None` exactly once for a request matching no file.
type FileMatch<'a> = Option<Query<(Entity, &'a SourceText), Where<BowlEq<FilePath>>>>;

/// One walk per request: definitions and spreads classify by kind, and
/// selection, clause, and predicate names by what they resolve to against
/// their context table. Ambient views of lowered facts put it at Complete.
async fn answer_semantic_tokens(
    requests: Query<(Entity, &FilePath), With<SemanticTokensRequest>>,
    file: FileMatch<'_>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    defs: View<'_, (Entity, &DefDecl, &NodeKey, &BelongsToFile)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (request, _path) = requests.item();
    let Some(file) = file else {
        commands.entity(request).insert(SemanticTokens(Vec::new()));
        return;
    };
    let (file_entity, _text) = file.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let tree = SelectionTree::collect(&views);
    let mut tokens = Vec::new();

    for (def_entity, decl, def_key, def_file) in defs.iter() {
        if def_file.0 != file_entity {
            continue;
        }
        tokens.push(SemanticToken {
            span: decl.name_span,
            kind: SemanticTokenKind::Fragment,
        });

        // A fragment's body resolves against its declared target; a
        // query's roots each name their own table.
        if let Some((_, _, target, _, _)) = tree
            .fragments
            .iter()
            .find(|(entity, _, _, _, _)| *entity == def_entity)
        {
            qualified_ref_tokens(
                &mut tokens,
                &target.name,
                target.span,
                SemanticTokenKind::Table,
            );
            if let Some(table) = catalog.table_ref_for(TableRef::parse(&target.name)) {
                selection_tokens(&mut tokens, &tree, catalog, table.id, table.id, *def_key);
            }
        } else {
            for (_, field, key, _) in tree.fields_under(*def_key) {
                let key = *key;
                qualified_ref_tokens(
                    &mut tokens,
                    &field.name,
                    field.name_span,
                    SemanticTokenKind::Table,
                );
                if let Some(table) = catalog.table_ref_for(TableRef::parse(&field.name)) {
                    clause_tokens(&mut tokens, &tree, catalog, table.id, table.id, key);
                    selection_tokens(&mut tokens, &tree, catalog, table.id, table.id, key);
                }
            }
        }
    }

    tokens.sort_by_key(|token| (token.span.start, token.span.end));
    tokens.dedup();
    commands.entity(request).insert(SemanticTokens(tokens));
}

/// Classifies one selection set against its context table, recursing into
/// resolved relations with their target table as the new context.
fn selection_tokens(
    tokens: &mut Vec<SemanticToken>,
    tree: &SelectionTree<'_>,
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    parent: NodeKey,
) {
    for (_, field, key, _) in tree.fields_under(parent) {
        let key = *key;
        if let Some(alias_span) = field.alias_span {
            tokens.push(SemanticToken {
                span: alias_span,
                kind: SemanticTokenKind::Alias,
            });
        }
        let reference = FieldRef {
            target: TableRef::parse(&field.name),
            selector: field.relation_path.as_deref(),
        };
        match catalog.check_field_ref(table, reference) {
            FieldCheckResult::Column(_) => {
                tokens.push(SemanticToken {
                    span: field.name_span,
                    kind: SemanticTokenKind::Column,
                });
            }
            FieldCheckResult::Relation(relation) => {
                let relation_table = relation.table.id;
                qualified_ref_tokens(
                    tokens,
                    &field.name,
                    field.name_span,
                    SemanticTokenKind::Relation,
                );
                // Clauses on a relation field filter the relation's rows.
                clause_tokens(tokens, tree, catalog, root_table, relation_table, key);
                selection_tokens(tokens, tree, catalog, root_table, relation_table, key);
            }
            FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {}
        }
    }

    for (_, spread, _, _) in tree.spreads_under(parent) {
        tokens.push(SemanticToken {
            span: spread.name_span,
            kind: SemanticTokenKind::Fragment,
        });
    }
}

/// Order-by columns and predicate paths of one field's clause list.
fn clause_tokens(
    tokens: &mut Vec<SemanticToken>,
    tree: &SelectionTree<'_>,
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    parent: NodeKey,
) {
    for (_, clause, _, _) in tree.clauses_under(parent) {
        match clause {
            ClauseFact::Where { expr } => {
                expr_tokens(tokens, catalog, root_table, table, expr);
            }
            ClauseFact::OrderBy { items } => {
                for item in items {
                    let reference = FieldRef {
                        target: TableRef::parse(&item.field),
                        selector: None,
                    };
                    if matches!(
                        catalog.check_field_ref(table, reference),
                        FieldCheckResult::Column(_)
                    ) {
                        tokens.push(SemanticToken {
                            span: item.field_span,
                            kind: SemanticTokenKind::Column,
                        });
                    }
                }
            }
            ClauseFact::Limit { .. } | ClauseFact::Offset { .. } => {}
        }
    }
}

/// Predicate paths classify segment by segment: relation steps, then the
/// terminal column — stopping silently where resolution does.
fn expr_tokens(
    tokens: &mut Vec<SemanticToken>,
    catalog: &Catalog,
    root_table: TableId,
    table: TableId,
    expr: &Expr,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            expr_tokens(tokens, catalog, root_table, table, lhs);
            expr_tokens(tokens, catalog, root_table, table, rhs);
        }
        Expr::Path {
            anchor, segments, ..
        } => {
            let mut current = match anchor {
                PathAnchor::Current => table,
                PathAnchor::Root => root_table,
                // Parent scope is not resolvable at check time.
                PathAnchor::Parent => return,
            };
            let Some((last, relations)) = segments.split_last() else {
                return;
            };
            for segment in relations {
                let reference = FieldRef {
                    target: TableRef::parse(&segment.name),
                    selector: segment.relation_path.as_deref(),
                };
                let FieldCheckResult::Relation(relation) =
                    catalog.check_field_ref(current, reference)
                else {
                    return;
                };
                qualified_ref_tokens(
                    tokens,
                    &segment.name,
                    segment.span,
                    SemanticTokenKind::Relation,
                );
                current = relation.table.id;
            }
            let reference = FieldRef {
                target: TableRef::parse(&last.name),
                selector: last.relation_path.as_deref(),
            };
            if matches!(
                catalog.check_field_ref(current, reference),
                FieldCheckResult::Column(_)
            ) {
                tokens.push(SemanticToken {
                    span: last.span,
                    kind: SemanticTokenKind::Column,
                });
            }
        }
        Expr::Literal { .. } | Expr::Variable { .. } | Expr::Error { .. } => {}
    }
}

/// Splits a qualified reference's span into its parts: `schema::name`
/// classifies the schema separately, and a `->selector` tail stays
/// unstyled. The lowered name is the raw span text, so offsets line up.
fn qualified_ref_tokens(
    tokens: &mut Vec<SemanticToken>,
    raw: &str,
    span: Span,
    tail_kind: SemanticTokenKind,
) {
    let target_end = raw.find("->").unwrap_or(raw.len());
    let target = &raw[..target_end];
    if let Some(delimiter) = target.find("::") {
        tokens.push(SemanticToken {
            span: Span {
                start: span.start,
                end: span.start + delimiter,
            },
            kind: SemanticTokenKind::Schema,
        });
        tokens.push(SemanticToken {
            span: Span {
                start: span.start + delimiter + "::".len(),
                end: span.start + target_end,
            },
            kind: tail_kind,
        });
    } else {
        tokens.push(SemanticToken {
            span: Span {
                start: span.start,
                end: span.start + target_end,
            },
            kind: tail_kind,
        });
    }
}
