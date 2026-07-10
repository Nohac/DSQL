//! Semantic tokens service: classifies every name span of a file for
//! editor highlighting.
//!
//! Classification reads the resolver's facts, so it needs no walk: each
//! candidate system is a tracked join contributing the tokens of one fact
//! kind as a [`TokenChunk`] fact keyed by the document it belongs to.
//! Chunks derive while a [`TokensDemand`] singleton is armed and settle
//! maintains them incrementally — an edit re-derives only the changed
//! rows' chunks. Requests are answered by [`semantic_tokens`], which is a
//! pure scoop: no request entities, no per-request re-derivation. The
//! chunks are *scooped*, not folded: a whole-file answer has hundreds of
//! contributors, and a MutRef fold into one component makes every write
//! invalidate every other pair's memo — O(N) settle generations for N
//! chunks (a minute of CPU on a large file). Only names that actually
//! resolve are classified; broken references stay unstyled and the
//! diagnostics point at them instead.

use bowl::{
    Bowl, Commands, Component, DerivedFrom, Entity, Query, Singleton, SystemExt, View, With,
};

use crate::catalog::{Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, TableRef};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, FragmentTarget};
use crate::entities::expression::{Expr, PathAnchor};
use crate::entities::fragment_spread::ResolvedSpread;
use crate::facts::{BelongsToFile, Span};
use crate::resolution::{ClauseContext, ResolvedClause, ResolvedSelection, SelectionTarget};
use crate::source::FilePath;

/// Arms token classification: chunks derive for every resolved fact while
/// this singleton exists — no demand, no tokens. [`semantic_tokens`] arms
/// it on first use.
#[derive(Component, Hash)]
#[component(hash)]
pub struct TokensDemand;

/// What a classified span highlights as. Ordered so equal-span tokens
/// merge deterministically.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
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

/// One fact's tokens, addressed at its document by [`BelongsToFile`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct TokenChunk(pub Vec<SemanticToken>);

/// Answers semantic tokens for one file path by scooping the chunk facts
/// of every document the path holds — the file itself, or each extracted
/// region of an embedding host with spans shifted back into host
/// coordinates — into one span-sorted, deduplicated list. Arms
/// [`TokensDemand`] on first use; an unknown path answers with no tokens.
pub async fn semantic_tokens(bowl: &Bowl, path: impl Into<String>) -> Vec<SemanticToken> {
    use crate::source::{BelongsToHost, SourceOffset};

    let armed = bowl.scoop::<Query<(Entity, &TokensDemand)>>().await;
    if armed.is_empty() {
        bowl.insert((Singleton::<TokensDemand>::new(), TokensDemand))
            .await;
    }

    let path = path.into();
    let files = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let file = files
        .collect()
        .into_iter()
        .find(|(_, candidate)| candidate.0 == path)
        .map(|(entity, _)| entity);

    // The documents the path answers for: its regions if it is an
    // embedding host, otherwise the file itself.
    let mut documents: Vec<(Entity, usize)> = Vec::new();
    if let Some(host) = file {
        let rows = bowl
            .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
            .await;
        documents = rows
            .collect()
            .into_iter()
            .filter(|(_, of, _)| of.0 == host)
            .map(|(region, _, offset)| (region, offset.0))
            .collect();
        if documents.is_empty() {
            documents.push((host, 0));
        }
    }

    let rows = bowl
        .scoop::<Query<(Entity, &TokenChunk, &BelongsToFile)>>()
        .await;
    let rows = rows.collect();
    let mut tokens: Vec<SemanticToken> = Vec::new();
    for (document, offset) in documents {
        tokens.extend(
            rows.iter()
                .filter(|(_, _, file)| file.0 == document)
                .flat_map(|(_, chunk, _)| chunk.0.iter())
                .map(|token| SemanticToken {
                    span: Span {
                        start: offset + token.span.start,
                        end: offset + token.span.end,
                    },
                    kind: token.kind,
                }),
        );
    }
    tokens.sort_by_key(|token| (token.span.start, token.span.end, token.kind));
    tokens.dedup();
    tokens
}

pub(crate) async fn register_semantic_tokens_pipeline(bowl: &Bowl) {
    bowl.add_system(definition_tokens).await;
    bowl.add_system(selection_tokens).await;
    bowl.add_system(spread_tokens).await;
    // Reads lowered clause facts ambiently: behind the Complete barrier.
    bowl.add_system(clause_tokens.run_during(bowl::Phase::Complete))
        .await;
}

/// Definition names highlight as fragments; a fragment's resolvable `on`
/// target as a table.
async fn definition_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    defs: Query<(Entity, &DefDecl, Option<&FragmentTarget>, &BelongsToFile)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let demand_entity = demand.item();
    let (def_entity, decl, target, file) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let mut tokens = vec![SemanticToken {
        span: decl.name_span,
        kind: SemanticTokenKind::Fragment,
    }];
    if let Some(target) = target
        && catalog
            .table_ref_for(TableRef::parse(&target.name))
            .is_some()
    {
        qualified_ref_tokens(
            &mut tokens,
            &target.name,
            target.span,
            SemanticTokenKind::Table,
        );
    }
    emit_chunk(
        &mut commands,
        DerivedFrom::many([def_entity, catalog_entity, demand_entity]),
        file.0,
        tokens,
    );
}

/// Selections classify by what they resolved to; aliases as aliases.
async fn selection_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    resolutions: Query<(Entity, &ResolvedSelection, &BelongsToFile)>,
    mut commands: Commands,
) {
    let demand_entity = demand.item();
    let (resolution_entity, resolved, file) = resolutions.item();

    let mut tokens = Vec::new();
    if let Some(alias_span) = resolved.alias_span {
        tokens.push(SemanticToken {
            span: alias_span,
            kind: SemanticTokenKind::Alias,
        });
    }
    match &resolved.target {
        SelectionTarget::Table(_) => qualified_ref_tokens(
            &mut tokens,
            &resolved.name,
            resolved.name_span,
            SemanticTokenKind::Table,
        ),
        SelectionTarget::Column(_) => tokens.push(SemanticToken {
            span: resolved.name_span,
            kind: SemanticTokenKind::Column,
        }),
        SelectionTarget::Relation { .. } => qualified_ref_tokens(
            &mut tokens,
            &resolved.name,
            resolved.name_span,
            SemanticTokenKind::Relation,
        ),
        SelectionTarget::Unresolved => {}
    }
    emit_chunk(
        &mut commands,
        DerivedFrom::many([resolution_entity, demand_entity]),
        file.0,
        tokens,
    );
}

/// Spread names highlight as fragments.
async fn spread_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    spreads: Query<(Entity, &ResolvedSpread, &BelongsToFile)>,
    mut commands: Commands,
) {
    let demand_entity = demand.item();
    let (spread_entity, resolved, file) = spreads.item();
    emit_chunk(
        &mut commands,
        DerivedFrom::many([spread_entity, demand_entity]),
        file.0,
        vec![SemanticToken {
            span: resolved.name_span,
            kind: SemanticTokenKind::Fragment,
        }],
    );
}

/// Order-by columns and predicate paths of one clause, against its
/// resolved context.
async fn clause_tokens(
    demand: Query<Entity, With<TokensDemand>>,
    resolutions: Query<(Entity, &ResolvedClause, &BelongsToFile)>,
    clauses: View<'_, (Entity, &ClauseFact)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let demand_entity = demand.item();
    let (resolution_entity, resolved, file) = resolutions.item();
    let (catalog_entity, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let Some(context) = resolved.context else {
        return;
    };
    // Evaluate-lowered fact referenced by entity id off the tracked
    // resolution row; the ambient lookup is why this system sits at
    // Complete.
    let Some((_, clause)) = clauses
        .iter()
        .find(|(entity, _)| *entity == resolved.clause)
    else {
        return;
    };

    let mut tokens = Vec::new();
    match clause {
        ClauseFact::Where { expr } => expr_tokens(&mut tokens, catalog, context, expr),
        ClauseFact::OrderBy { items } => {
            for item in items {
                let reference = FieldRef {
                    target: TableRef::parse(&item.field),
                    selector: None,
                };
                if matches!(
                    catalog.check_field_ref(context.table, reference),
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
    emit_chunk(
        &mut commands,
        DerivedFrom::many([resolution_entity, catalog_entity, demand_entity]),
        file.0,
        tokens,
    );
}

fn emit_chunk(
    commands: &mut Commands,
    anchor: DerivedFrom,
    file: Entity,
    tokens: Vec<SemanticToken>,
) {
    if tokens.is_empty() {
        return;
    }
    commands.insert((anchor, BelongsToFile(file), TokenChunk(tokens)));
}

/// Predicate paths classify segment by segment: relation steps, then the
/// terminal column — stopping silently where resolution does.
fn expr_tokens(
    tokens: &mut Vec<SemanticToken>,
    catalog: &Catalog,
    context: ClauseContext,
    expr: &Expr,
) {
    match expr {
        Expr::Binary { lhs, rhs, .. } => {
            expr_tokens(tokens, catalog, context, lhs);
            expr_tokens(tokens, catalog, context, rhs);
        }
        Expr::Path {
            anchor, segments, ..
        } => {
            let mut current = match anchor {
                PathAnchor::Current => context.table,
                PathAnchor::Root => context.root,
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
