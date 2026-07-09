//! Semantic tokens service: classifies every name span of a file for
//! editor highlighting.
//!
//! Classification reads the resolver's facts, so it needs no walk: each
//! candidate system is a tracked join contributing the tokens of one fact
//! kind as a [`TokenChunk`] fact. The chunks are *scooped*, not folded:
//! a whole-file answer has hundreds of contributors, and a MutRef fold
//! into one component makes every write invalidate every other pair's
//! memo — O(N) settle generations for N chunks (a minute of CPU on a
//! large file). Bulk aggregation belongs at the read boundary; requests
//! are answered by [`semantic_tokens`]. Only names that actually resolve
//! are classified; broken references stay unstyled and the diagnostics
//! point at them instead.

use bowl::{
    Bowl, Commands, Component, DerivedFrom, Entity, Eq as BowlEq, Query, View, Where, With,
};

use crate::catalog::{Catalog, CatalogSnapshot, FieldCheckResult, FieldRef, TableRef};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, FragmentTarget};
use crate::entities::expression::{Expr, PathAnchor};
use crate::entities::fragment_spread::ResolvedSpread;
use crate::facts::{BelongsToFile, Span};
use crate::resolution::{ClauseContext, ResolvedClause, ResolvedSelection, SelectionTarget};
use crate::service::hover::RequestKey;
use crate::source::{FilePath, SourceText};

/// Marks an entity as a semantic-tokens request; pair with [`FilePath`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct SemanticTokensRequest;

/// Addresses a request at a specific document entity instead of a path —
/// how [`semantic_tokens`] asks for one extracted region of a host file.
#[derive(Component, Hash, Debug, Clone, Copy)]
#[component(hash)]
pub struct TargetFile(pub Entity);

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

/// One fact's contribution to one request, addressed by an equal
/// [`RequestKey`].
#[derive(Component, Hash)]
#[component(hash)]
pub struct TokenChunk(pub Vec<SemanticToken>);

/// Answers semantic tokens for one file path: inserts a request per
/// document the path holds — the file itself, or each extracted region of
/// an embedding host with spans shifted back into host coordinates — and
/// gathers the chunk facts into one span-sorted, deduplicated list. An
/// unknown path answers with no tokens.
pub async fn semantic_tokens(bowl: &Bowl, path: impl Into<String>) -> Vec<SemanticToken> {
    use crate::source::{BelongsToHost, FilePath, SourceOffset};

    let path = path.into();
    let files = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let file = files
        .collect()
        .into_iter()
        .find(|(_, candidate)| candidate.0 == path)
        .map(|(entity, _)| entity);
    let regions: Vec<(Entity, usize)> = match file {
        Some(host) => {
            let rows = bowl
                .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
                .await;
            rows.collect()
                .into_iter()
                .filter(|(_, of, _)| of.0 == host)
                .map(|(region, _, offset)| (region, offset.0))
                .collect()
        }
        None => Vec::new(),
    };

    // One request per document; plain files are their own document.
    let mut requests: Vec<(Entity, usize)> = Vec::new();
    if regions.is_empty() {
        let request = bowl
            .insert((SemanticTokensRequest, FilePath(path)))
            .await
            .entity();
        requests.push((request, 0));
    } else {
        for (region, offset) in regions {
            let request = bowl
                .insert((
                    SemanticTokensRequest,
                    FilePath(path.clone()),
                    TargetFile(region),
                ))
                .await
                .entity();
            requests.push((request, offset));
        }
    }

    let rows = bowl
        .scoop::<Query<(Entity, &TokenChunk, &RequestKey)>>()
        .await;
    let rows = rows.collect();
    let mut tokens: Vec<SemanticToken> = Vec::new();
    for (request, offset) in requests {
        tokens.extend(
            rows.iter()
                .filter(|(_, _, key)| key.0 == request)
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
    bowl.add_system(resolve_token_requests).await;
    bowl.add_system(definition_tokens).await;
    bowl.add_system(selection_tokens).await;
    bowl.add_system(spread_tokens).await;
    bowl.add_system(clause_tokens).await;
}

/// The file side of the request outer join: matched per equal path, or
/// `None` exactly once for a request matching no file.
type FileMatch<'a> = Option<Query<(Entity, &'a SourceText), Where<BowlEq<FilePath>>>>;

/// Definition rows of the request's file, targets riding along.
type DefRows<'a> =
    Query<(Entity, &'a DefDecl, Option<&'a FragmentTarget>), Where<BowlEq<BelongsToFile>>>;

/// Outer join: stamps the request key, and the target document as
/// `BelongsToFile` for the candidate joins — an explicit [`TargetFile`]
/// wins over the path join, which is how region requests are addressed
/// (regions carry no [`FilePath`]).
async fn resolve_token_requests(
    requests: Query<(Entity, &FilePath, Option<&TargetFile>), With<SemanticTokensRequest>>,
    file: FileMatch<'_>,
    mut commands: Commands,
) {
    let (request, _path, target) = requests.item();
    commands.entity(request).insert(RequestKey(request));
    if let Some(target) = target {
        commands.entity(request).insert(BelongsToFile(target.0));
    } else if let Some(file) = file {
        let (file_entity, _text) = file.item();
        commands.entity(request).insert(BelongsToFile(file_entity));
    }
}

/// Definition names highlight as fragments; a fragment's resolvable `on`
/// target as a table.
async fn definition_tokens(
    requests: Query<(Entity, &BelongsToFile), With<SemanticTokensRequest>>,
    defs: DefRows<'_>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let (request, _file) = requests.item();
    let (_, decl, target) = defs.item();
    let (_, snapshot) = catalog.item();
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
    emit_chunk(&mut commands, request, tokens);
}

/// Selections classify by what they resolved to; aliases as aliases.
async fn selection_tokens(
    requests: Query<(Entity, &BelongsToFile), With<SemanticTokensRequest>>,
    resolutions: Query<(Entity, &ResolvedSelection), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands,
) {
    let (request, _file) = requests.item();
    let (_, resolved) = resolutions.item();

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
    emit_chunk(&mut commands, request, tokens);
}

/// Spread names highlight as fragments.
async fn spread_tokens(
    requests: Query<(Entity, &BelongsToFile), With<SemanticTokensRequest>>,
    spreads: Query<(Entity, &ResolvedSpread), Where<BowlEq<BelongsToFile>>>,
    mut commands: Commands,
) {
    let (request, _file) = requests.item();
    let (_, resolved) = spreads.item();
    emit_chunk(
        &mut commands,
        request,
        vec![SemanticToken {
            span: resolved.name_span,
            kind: SemanticTokenKind::Fragment,
        }],
    );
}

/// Order-by columns and predicate paths of one clause, against its
/// resolved context.
async fn clause_tokens(
    requests: Query<(Entity, &BelongsToFile), With<SemanticTokensRequest>>,
    resolutions: Query<(Entity, &ResolvedClause), Where<BowlEq<BelongsToFile>>>,
    clauses: View<'_, (Entity, &ClauseFact)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    mut commands: Commands,
) {
    let (request, _file) = requests.item();
    let (_, resolved) = resolutions.item();
    let (_, snapshot) = catalog.item();
    let catalog = snapshot.catalog();

    let Some(context) = resolved.context else {
        return;
    };
    // Evaluate-lowered fact referenced by entity id off the tracked
    // resolution row; derived strictly after it, so race-free.
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
    emit_chunk(&mut commands, request, tokens);
}

fn emit_chunk(commands: &mut Commands, request: Entity, tokens: Vec<SemanticToken>) {
    if tokens.is_empty() {
        return;
    }
    commands.insert((
        DerivedFrom::new(request),
        RequestKey(request),
        TokenChunk(tokens),
    ));
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
