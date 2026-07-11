//! Marker-driven service scenarios, ported from the POC's LSP snapshot
//! suite: sources carry `<|>` cursor markers, and each scenario snapshots
//! the service answers (completion context and items, hover, definitions,
//! diagnostics) at those positions. Bowl-level on purpose — language
//! behavior is tested where it is fast and deterministic; transport
//! concerns live in the dsql-lsp protocol harness.

use bowl::{Bowl, Entity, Mut, Query};
use dsql_core::catalog::insert_catalog;
use dsql_core::service::{
    CompletionList, CompletionRequest, DefinitionRequest, DefinitionTarget, HoverInfo,
    HoverRequest, Position,
};
use dsql_core::source::{FilePath, SourceText, insert_source_scoped};
use futures::executor::block_on;

use crate::imdb_catalog;

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
        let rows = self.bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (entity, text) in rows.collect() {
            if entity == target {
                let clean = clean.clone();
                text.with_latest(move |text| text.set_text(&clean)).await;
            }
        }
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
                lines.push(format!(
                    "context: {:?} table={table} scope={}",
                    context.site, context.scope
                ));
            }
            None => lines.push("context: <none>".to_string()),
        }
        for item in &items.0 {
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

    /// Renders a definition answer as `path[start..end] -> "text"`.
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
        let rows = self
            .bowl
            .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
            .await;
        let (target_path, text) = rows
            .collect()
            .into_iter()
            .find(|(entity, _, _)| *entity == target.file)
            .map(|(_, path, text)| (path.0.clone(), text.to_text()))
            .unwrap_or_default();
        format!(
            "{target_path}[{}..{}] -> `{}`",
            target.span.start,
            target.span.end,
            &text[target.span.start..target.span.end]
        )
    }

    async fn diagnostics(&self) -> String {
        crate::render_diagnostic_facts(&self.bowl).await
    }
}

/// Completion directly after a selection set opens: columns, relations,
/// and the legal keywords of the position.
#[test]
fn completion_at_selection_start() {
    block_on(async {
        let scenario = Scenario::new().await;
        let markers = scenario
            .open(
                "s.dsql",
                "query Q {\n  title(limit 1) {\n    <|>\n    id\n  }\n}\n",
            )
            .await;
        insta::assert_snapshot!(scenario.complete("s.dsql", markers[0]).await);
    });
}

/// PINNED CURRENT DEFECT (heals in the context-resolution fix): the
/// site classifies correctly as `SelectionBody`, but the context table
/// resolves to `None`, so only grammar keywords answer instead of the
/// table's columns.
#[test]
fn completion_between_and_after_fields() {
    block_on(async {
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
    });
}

/// Spread completion offers visible fragments, across open documents.
#[test]
fn completion_of_spreads_across_documents() {
    block_on(async {
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
    });
}

/// PINNED CURRENT DEFECT (heals in the context-resolution fix): fragment
/// bodies must complete against the fragment's `on` table — today the
/// context table never resolves, so nothing answers.
#[test]
fn completion_inside_fragment_bodies() {
    block_on(async {
        let scenario = Scenario::new().await;
        let markers = scenario
            .open("f.dsql", "fragment Bits on kind_type {\n  <|>\n}\n")
            .await;
        insta::assert_snapshot!(scenario.complete("f.dsql", markers[0]).await);
    });
}

/// Definitions resolve across open documents: the spread's target lands
/// in the other file.
#[test]
fn definitions_resolve_across_documents() {
    block_on(async {
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
    });
}

/// Hover shows inferred variable bindings.
#[test]
fn hover_shows_variable_bindings() {
    block_on(async {
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
    });
}

/// Reopening a document with new text updates diagnostics — the editor's
/// full-buffer replace path.
#[test]
fn updates_rederive_diagnostics() {
    block_on(async {
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
    });
}

/// Embedded regions answer completion at host coordinates.
#[test]
fn completion_inside_embedded_regions() {
    block_on(async {
        let scenario = Scenario::new().await;
        let markers = scenario
            .open(
                "host.ts",
                "import { dsql } from \"./dsql\";\nexport const q = dsql`\nquery H {\n  title(limit 1) {\n    <|>\n  }\n}\n`;\n",
            )
            .await;
        insta::assert_snapshot!(scenario.complete("host.ts", markers[0]).await);
    });
}
