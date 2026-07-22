//! Marker-driven service scenarios, ported from the POC's LSP snapshot
//! suite: sources carry `<|>` cursor markers, and each scenario snapshots
//! the service answers (completion context and items, hover, definitions,
//! diagnostics) at those positions. Bowl-level on purpose — language
//! behavior is tested where it is fast and deterministic; transport
//! concerns live in the dsql-lsp protocol harness.

use bowl::{Bowl, Entity, Query};
use dsql_core::catalog::insert_catalog;
use dsql_core::service::{
    CompletionList, CompletionRequest, DefinitionRequest, DefinitionTarget, HoverInfo,
    HoverRequest, Position,
};
use dsql_core::source::{FilePath, SourceText, insert_source_scoped};

use crate::{imdb_catalog, set_source_text};

/// Strips `<|>` markers out of a source, returning the clean text and the
/// marker byte offsets in order.
fn marked(source: &str) -> (String, Vec<usize>) {
    let mut clean = String::with_capacity(source.len());
    let mut offsets = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("<|>") {
        clean.push_str(&rest[..at]);
        offsets.push(clean.len());
        rest = &rest[at + 3..];
    }
    clean.push_str(rest);
    (clean, offsets)
}

struct Scenario {
    bowl: Bowl,
}

impl Scenario {
    async fn new() -> Self {
        let bowl = dsql_core::language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        dsql_core::facts::arm_editor_demands(&bowl).await;
        Self { bowl }
    }

    /// Opens `source` (markers stripped) under `path` in `scope`,
    /// returning the marker offsets.
    async fn open_scoped(&self, path: &str, scope: &str, source: &str) -> Vec<usize> {
        let (clean, offsets) = marked(source);
        insert_source_scoped(
            &self.bowl,
            path,
            &clean,
            dsql_core::source::ResolutionScope(scope.to_string()),
            dsql_core::source::SourceKind::Dsql,
        )
        .await;
        offsets
    }

    async fn open(&self, path: &str, source: &str) -> Vec<usize> {
        self.open_scoped(path, dsql_core::source::ResolutionScope::DEFAULT, source)
            .await
    }

    /// Reopens/updates `path` with new text (markers stripped), the way an
    /// editor buffer replaces content.
    async fn update(&self, path: &str, source: &str) -> Vec<usize> {
        let (clean, offsets) = marked(source);
        let target = self.file(path).await;
        set_source_text(&self.bowl, target, clean).await;
        offsets
    }

    async fn file(&self, path: &str) -> Entity {
        let rows = self.bowl.scoop::<Query<(Entity, &FilePath)>>().await;
        rows.collect()
            .into_iter()
            .find(|(_, candidate)| candidate.0 == path)
            .map(|(entity, _)| entity)
            .expect("scenario file exists")
    }

    /// Renders the completion answer at one marker: the classified
    /// context, then every item with its kind, detail, and insert text.
    async fn complete(&self, path: &str, offset: usize) -> String {
        use dsql_core::service::CompletionContext;

        let catalog = imdb_catalog();

        let inserted = self
            .bowl
            .insert((
                CompletionRequest,
                FilePath(path.to_string()),
                Position { offset },
            ))
            .await;
        let request = inserted.entity();
        // Scoop the context stamp BEFORE taking the list (a take bumps
        // the request entity, reaping the derived context stamp) — and
        // inside a scope, because a live QueryResult pins the entity's
        // cells and the take would spin forever waiting for it to drop.
        let context = {
            let contexts = self
                .bowl
                .scoop::<Query<(Entity, &CompletionContext)>>()
                .await;
            contexts
                .collect()
                .into_iter()
                .find(|(entity, _)| *entity == request)
                .map(|(_, context)| context.clone())
        };
        let items = inserted
            .bind()
            .take::<CompletionList>()
            .await
            .expect("completion answers");

        let mut lines = Vec::new();
        match context {
            Some(context) => {
                let table = context
                    .table
                    .and_then(|table| catalog.table_by_id(table))
                    .map_or("<unresolved>".to_string(), |table| table.name.clone());
                let replace = items.replace.map_or("<none>".to_string(), |span| {
                    format!("[{}..{}]", span.start, span.end)
                });
                lines.push(format!(
                    "context: {:?} table={table} scope={} replace={replace}",
                    context.site, context.scope
                ));
            }
            None => lines.push("context: <none>".to_string()),
        }
        for item in &items.items {
            let mut line = format!("{:?} {}", item.kind, item.label);
            if let Some(detail) = &item.detail {
                line.push_str(&format!(" — {detail}"));
            }
            if let Some(insert) = &item.insert_text {
                line.push_str(&format!(" (inserts `{insert}`)"));
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    async fn hover(&self, path: &str, offset: usize) -> String {
        let info = self
            .bowl
            .insert((
                HoverRequest,
                FilePath(path.to_string()),
                Position { offset },
            ))
            .await
            .bind()
            .take::<HoverInfo>()
            .await
            .expect("hover answers");
        info.text.clone()
    }

    /// Renders a source definition as `path[start..end] -> "text"` and a
    /// catalog definition as its stable semantic identity.
    async fn definition(&self, path: &str, offset: usize) -> String {
        let target = self
            .bowl
            .insert((
                DefinitionRequest,
                FilePath(path.to_string()),
                Position { offset },
            ))
            .await
            .bind()
            .take::<DefinitionTarget>()
            .await;
        let Ok(target) = target else {
            return "<no definition>".to_string();
        };
        match target.as_ref() {
            DefinitionTarget::Catalog(target) => format!("{target:?}"),
            DefinitionTarget::Source { file, span } => {
                let rows = self
                    .bowl
                    .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
                    .await;
                let (target_path, text) = rows
                    .collect()
                    .into_iter()
                    .find(|(entity, _, _)| entity == file)
                    .map(|(_, path, text)| {
                        (
                            path.0.clone(),
                            text.to_text().expect("scenario text is resident"),
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "{target_path}[{}..{}] -> `{}`",
                    span.start,
                    span.end,
                    &text[span.start..span.end]
                )
            }
        }
    }

    async fn diagnostics(&self) -> String {
        crate::render_diagnostic_facts(&self.bowl).await
    }
}

/// Completion directly after a selection set opens: columns, relations,
/// and the legal keywords of the position.
#[tokio::test]
async fn completion_at_selection_start() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open(
            "s.dsql",
            "query Q {\n  title(limit 1) {\n    <|>\n    id\n  }\n}\n",
        )
        .await;
    insta::assert_snapshot!(scenario.complete("s.dsql", markers[0]).await);
}

#[tokio::test]
async fn policy_services_follow_scope_and_collection_targets() {
    use dsql_core::service::semantic_tokens;

    let scenario = Scenario::new().await;
    let policy_source = indoc::indoc! {r#"
        condition <|>Admin { where $:is_admin }
        filter <|>TitleAccess on title {
          apply where <|>Admin
          field production_year where <|>Admin
        }
        filter NameAccess on name { where .id > 0 }
        query Q(filter <|>TitleAccess when false) {
          title(filter <|>TitleAccess) { id }
        }
    "#};
    let markers = scenario.open("policies.dsql", policy_source).await;
    let (clean, _) = marked(policy_source);
    let tokens = semantic_tokens(&scenario.bowl, "policies.dsql")
        .await
        .into_iter()
        .filter(|token| token.kind == dsql_core::service::SemanticTokenKind::Policy)
        .map(|token| {
            format!(
                "{}..{} `{}`",
                token.span.start,
                token.span.end,
                &clean[token.span.start..token.span.end]
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let header_completion = scenario
        .open("header.dsql", "query Header(filter T<|>) { title { id } }")
        .await;
    let nested_completion = scenario
        .open("nested.dsql", "query Nested { title(filter <|>) { id } }")
        .await;

    insta::assert_snapshot!(format!(
        "condition declaration hover:\n{}\n\ncondition reference hover:\n{}\n\nfilter assignment hover:\n{}\n\ncondition definition:\n{}\n\nfilter definition:\n{}\n\nheader completion:\n{}\n\nnested completion:\n{}\n\npolicy tokens:\n{tokens}",
        scenario.hover("policies.dsql", markers[0]).await,
        scenario.hover("policies.dsql", markers[2]).await,
        scenario.hover("policies.dsql", markers[5]).await,
        scenario.definition("policies.dsql", markers[3]).await,
        scenario.definition("policies.dsql", markers[4]).await,
        scenario.complete("header.dsql", header_completion[0]).await,
        scenario.complete("nested.dsql", nested_completion[0]).await,
    ));
}

/// Completion between and after sibling fields resolves the enclosing
/// set's table — containment is decided by the selection set's braces,
/// not the preceding field's span (which swallows trailing trivia).
#[tokio::test]
async fn completion_between_and_after_fields() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open(
            "s.dsql",
            "query Q {\n  title(limit 1) {\n    id\n    <|>\n    title\n    <|>\n  }\n}\n",
        )
        .await;
    let between = scenario.complete("s.dsql", markers[0]).await;
    let after = scenario.complete("s.dsql", markers[1]).await;
    insta::assert_snapshot!(format!("between:\n{between}\n\nafter:\n{after}"));
}

/// Spread completion offers visible fragments, across open documents.
#[tokio::test]
async fn completion_of_spreads_across_documents() {
    let scenario = Scenario::new().await;
    scenario
        .open("frags.dsql", "fragment Bits on title {\n  id\n}\n")
        .await;
    let markers = scenario
        .open(
            "q.dsql",
            "query Q {\n  title(limit 1) {\n    ...<|>\n  }\n}\n",
        )
        .await;
    insta::assert_snapshot!(scenario.complete("q.dsql", markers[0]).await);
}

/// Fragment bodies complete against the fragment's `on` table.
#[tokio::test]
async fn completion_inside_fragment_bodies() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open("f.dsql", "fragment Bits on kind_type {\n  <|>\n}\n")
        .await;
    insta::assert_snapshot!(scenario.complete("f.dsql", markers[0]).await);
}

/// Partially typed spreads keep the dots already written: fragment items
/// insert only the missing dots, so accepting after `.`, `..`, or a
/// half-typed `...Bi` always lands on a single `...Bits`.
#[tokio::test]
async fn completion_of_partial_spreads_inserts_missing_dots() {
    let scenario = Scenario::new().await;
    scenario
        .open("frags.dsql", "fragment Bits on title {\n  id\n}\n")
        .await;
    let markers = scenario
            .open(
                "q.dsql",
                "query Q {\n  title(limit 1) {\n    .<|>\n  }\n  title(limit 1) {\n    ..<|>\n  }\n  title(limit 1) {\n    .Bi<|>\n  }\n  title(limit 1) {\n    ..Bi<|>\n  }\n  title(limit 1) {\n    ...Bi<|>\n  }\n}\n",
            )
            .await;
    let mut sections = Vec::new();
    for (label, offset) in [
        "one dot",
        "two dots",
        "one dot partial name",
        "two dots partial name",
        "three dots partial name",
    ]
    .iter()
    .zip(&markers)
    {
        sections.push(format!(
            "{label}:\n{}",
            scenario.complete("q.dsql", *offset).await
        ));
    }
    insta::assert_snapshot!(sections.join("\n\n"));
}

/// Directive positions complete from the registry: the namespace after
/// `@`, members after `@dsql.` and the `@.` shorthand, argument names
/// inside the parens (after `(` and after `,`), and boolean values after
/// `:` — with no generic expression keywords leaking in (no `null`).
#[tokio::test]
async fn completion_of_directives_follows_the_registry() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open(
            "d.dsql",
            indoc::indoc! {r#"
                query D {
                  title(limit 1) {
                    a: id @<|>
                    b: id @dsql.<|>
                    c: id @.<|>
                    d: id @dsql.include_if(<|>)
                    e: id @dsql.include_if(if: <|>)
                    f: id @dsql.deprecated(reason: <|>)
                    g: id @dsql.include_if(if: true, <|>)
                    h: id @dsql.include_if(if <|>)
                    i: id @dsql.include_if( # a comment
                      <|>)
                  }
                }
            "#},
        )
        .await;
    let mut sections = Vec::new();
    for (label, offset) in [
        "at @",
        "after dsql.",
        "after . shorthand",
        "argument name",
        "boolean value",
        "string value",
        "after comma",
        "name without colon",
        "after a comment",
    ]
    .iter()
    .zip(&markers)
    {
        sections.push(format!(
            "{label}:\n{}",
            scenario.complete("d.dsql", *offset).await
        ));
    }
    insta::assert_snapshot!(sections.join("\n\n"));
}

/// Definitions resolve across open documents: the spread's target lands
/// in the other file.
#[tokio::test]
async fn definitions_resolve_across_documents() {
    let scenario = Scenario::new().await;
    scenario
        .open("frags.dsql", "fragment Bits on title {\n  id\n}\n")
        .await;
    let markers = scenario
        .open(
            "q.dsql",
            "query Q {\n  title(limit 1) {\n    ...Bi<|>ts\n  }\n}\n",
        )
        .await;
    insta::assert_snapshot!(scenario.definition("q.dsql", markers[0]).await);
}

/// Catalog definitions consume the resolver's semantic facts for fragment
/// targets, query roots, predicate paths, order items, scalar columns, and
/// relation fields.
#[tokio::test]
async fn definitions_resolve_catalog_targets() {
    let scenario = Scenario::new().await;
    let markers = scenario
            .open(
                "catalog.dsql",
                indoc::indoc! {r#"
                    fragment Bits on ti<|>tle {
                      id
                    }
                    query Catalog {
                      movie_i<|>nfo(where .ti<|>tle.production_y<|>ear == 2000 order by no<|>te asc) {
                        no<|>te
                        ti<|>tle {
                          id
                        }
                      }
                    }
                "#},
            )
            .await;
    let mut answers = Vec::new();
    for (label, offset) in [
        "fragment target",
        "query root",
        "predicate relation",
        "predicate column",
        "order column",
        "selected column",
        "selected relation",
    ]
    .iter()
    .zip(markers)
    {
        answers.push(format!(
            "{label}: {}",
            scenario.definition("catalog.dsql", offset).await
        ));
    }
    insta::assert_snapshot!(answers.join("\n"));
}

/// Hover shows inferred variable bindings.
#[tokio::test]
async fn hover_shows_variable_bindings() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open(
            "v.dsql",
            "query V {\n  title(where .production_year == $ye<|>ar limit 1) {\n    id\n  }\n}\n",
        )
        .await;
    let hover = scenario.hover("v.dsql", markers[0]).await;
    assert!(
        hover.contains("year"),
        "hover names the variable, got {hover:?}"
    );
    insta::assert_snapshot!(hover);
}

/// Reopening a document with new text updates diagnostics — the editor's
/// full-buffer replace path.
#[tokio::test]
async fn updates_rederive_diagnostics() {
    let scenario = Scenario::new().await;
    scenario
        .open("u.dsql", "query U {\n  title(limit 1) {\n    id\n  }\n}\n")
        .await;
    assert_eq!(scenario.diagnostics().await, "");

    scenario
        .update(
            "u.dsql",
            "query U {\n  title(limit 1) {\n    bogus\n  }\n}\n",
        )
        .await;
    insta::assert_snapshot!(scenario.diagnostics().await);
}

/// Completion mid-identifier reports the identifier's span as the range
/// accepting an item replaces, from every cursor position in the word.
#[tokio::test]
async fn completion_mid_identifier_replaces_the_word() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open(
            "s.dsql",
            "query Q {\n  title(limit 1) {\n    <|>ki<|>nd<|>\n  }\n}\n",
        )
        .await;
    let mut sections = Vec::new();
    for (label, offset) in ["start", "middle", "end"].iter().zip(&markers) {
        sections.push(format!(
            "{label}:\n{}",
            scenario.complete("s.dsql", *offset).await
        ));
    }
    insta::assert_snapshot!(sections.join("\n\n"));
}

/// An empty selection set completes columns and relations with no
/// identifier to replace.
#[tokio::test]
async fn completion_in_empty_selection_sets() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open("s.dsql", "query Q {\n  title(limit 1) {<|>}\n}\n")
        .await;
    insta::assert_snapshot!(scenario.complete("s.dsql", markers[0]).await);
}

/// Completion inside clause lists and where-expressions between fields —
/// the clause context resolves against the owning field's table.
#[tokio::test]
async fn completion_inside_clauses() {
    let scenario = Scenario::new().await;
    let markers = scenario
            .open(
                "s.dsql",
                "query Q {\n  title(<|>limit 1) {\n    id\n  }\n  title(where .<|> limit 1) {\n    id\n  }\n}\n",
            )
            .await;
    let clause_list = scenario.complete("s.dsql", markers[0]).await;
    let where_anchor = scenario.complete("s.dsql", markers[1]).await;
    insta::assert_snapshot!(format!(
        "clause list:\n{clause_list}\n\nwhere anchor:\n{where_anchor}"
    ));
}

/// Malformed sources still answer with full context: a missing closing
/// brace, a dangling spread, and an incomplete clause all resolve their
/// enclosing field's table — the truncated parse ends at the cursor, so
/// its open constructs are exactly what is being typed into.
#[tokio::test]
async fn completion_survives_malformed_sources() {
    let scenario = Scenario::new().await;
    let missing_brace = scenario
        .open("broken1.dsql", "query Q {\n  title(limit 1) {\n    <|>\n")
        .await;
    let dangling_spread = scenario
        .open(
            "broken2.dsql",
            "query Q {\n  title(limit 1) {\n    ...<|>\n",
        )
        .await;
    let incomplete_clause = scenario
        .open("broken3.dsql", "query Q {\n  title(where <|>\n")
        .await;
    let brace = scenario.complete("broken1.dsql", missing_brace[0]).await;
    let spread = scenario.complete("broken2.dsql", dangling_spread[0]).await;
    let clause = scenario
        .complete("broken3.dsql", incomplete_clause[0])
        .await;
    insta::assert_snapshot!(format!(
        "missing brace:\n{brace}\n\ndangling spread:\n{spread}\n\nincomplete clause:\n{clause}"
    ));
}

/// A cursor immediately after a nested set's closing brace sits in the
/// enclosing selection — the finished set does not capture it.
#[tokio::test]
async fn completion_after_nested_closing_brace() {
    let scenario = Scenario::new().await;
    let markers = scenario
        .open(
            "s.dsql",
            "query Q {\n  title(limit 1) {\n    id\n  }<|>\n}\n",
        )
        .await;
    insta::assert_snapshot!(scenario.complete("s.dsql", markers[0]).await);
}

/// Completion at the very end of the document.
#[tokio::test]
async fn completion_at_end_of_file() {
    let scenario = Scenario::new().await;
    let markers = scenario.open("s.dsql", "query Q {\n  title\n}\n<|>").await;
    insta::assert_snapshot!(scenario.complete("s.dsql", markers[0]).await);
}

/// Every byte offset of a representative document classifies to a sane
/// context — the sweep pins the exact site/table/replace boundaries and
/// catches positions that would panic or misclassify.
#[tokio::test]
async fn completion_context_sweeps_every_offset() {
    let scenario = Scenario::new().await;
    scenario
        .open("frags.dsql", "fragment Bits on title {\n  id\n}\n")
        .await;
    let source = "query Q {\n  title(where .production_year >= 2000 limit 1) {\n    id\n    ...Bits\n  }\n}\n";
    scenario.open("s.dsql", source).await;

    // Dedup consecutive offsets sharing a context into ranges.
    let mut runs: Vec<(usize, usize, String)> = Vec::new();
    for offset in 0..=source.len() {
        let answer = scenario.complete("s.dsql", offset).await;
        let context = answer.lines().next().unwrap_or_default().to_string();
        match runs.last_mut() {
            Some((_, end, line)) if *line == context && *end + 1 == offset => *end = offset,
            _ => runs.push((offset, offset, context)),
        }
    }
    let rendered: Vec<String> = runs
        .into_iter()
        .map(|(start, end, line)| {
            format!(
                "[{start:>2}..{end:>2}] {:?} {line}",
                &source[start..end.min(source.len())]
            )
        })
        .collect();
    insta::assert_snapshot!(rendered.join("\n"));
}

/// Embedded regions answer completion at host coordinates.
#[tokio::test]
async fn completion_inside_embedded_regions() {
    let scenario = Scenario::new().await;
    let markers = scenario
            .open(
                "host.ts",
                "import { dsql } from \"./dsql\";\nexport const q = dsql`\nquery H {\n  title(limit 1) {\n    <|>\n  }\n}\n`;\n",
            )
            .await;
    insta::assert_snapshot!(scenario.complete("host.ts", markers[0]).await);
}

/// A cursor at an embedded region's final byte (right before the closing
/// delimiter) still belongs to the region — the boundary is inclusive.
#[tokio::test]
async fn completion_at_embedded_region_end() {
    let scenario = Scenario::new().await;
    let markers = scenario
            .open(
                "host.ts",
                "import { dsql } from \"./dsql\";\nexport const q = dsql`\nquery H {\n  title\n}\n<|>`;\n",
            )
            .await;
    insta::assert_snapshot!(scenario.complete("host.ts", markers[0]).await);
}
