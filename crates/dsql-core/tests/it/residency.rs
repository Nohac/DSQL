//! Hash-carrying evictable ropes (docs/issues.md): batch analysis drops
//! source ropes once the parse boundary has materialized what
//! derivations need, while the stored content hash keeps every derived
//! fact valid — eviction is fingerprint-neutral, rehydration with equal
//! content is too, and only real content changes re-derive.

use bowl::{Bowl, Entity, Mut, Query, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::facts::{Diagnostic, DiagnosticsDemand};
use dsql_core::language_bowl;
use dsql_core::source::{
    OpenBuffer, SourceText, arm_analysis_residency, insert_embedding_source, insert_source,
};
use futures::executor::block_on;

use crate::imdb_catalog;

const HOST: &str = "import { dsql } from \"./dsql\";\nexport const q = dsql`\nquery H {\n  title(limit 1) {\n    id\n  }\n}\n`;\n";

async fn analysis_bowl() -> Bowl {
    let bowl = language_bowl().await;
    arm_analysis_residency(&bowl).await;
    insert_catalog(&bowl, imdb_catalog()).await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl
}

/// Residency of every source text, as `(path-or-kind, resident)` pairs.
async fn residency(bowl: &Bowl) -> Vec<bool> {
    let rows = bowl.scoop::<Query<(Entity, &SourceText)>>().await;
    let mut states: Vec<bool> = rows
        .collect()
        .into_iter()
        .map(|(_, text)| text.is_resident())
        .collect();
    states.sort();
    states
}

async fn diagnostic_entities(bowl: &Bowl) -> Vec<Entity> {
    let rows = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await;
    let mut entities: Vec<Entity> = rows
        .collect()
        .into_iter()
        .map(|(entity, _)| entity)
        .collect();
    entities.sort();
    entities
}

/// Batch mode evicts plain documents, hosts, and regions alike; the
/// settle is stable (a second look changes nothing), and re-deriving is
/// not triggered by the eviction itself.
#[test]
fn analysis_mode_evicts_after_parse_without_rederiving() {
    block_on(async {
        let bowl = analysis_bowl().await;
        insert_source(
            &bowl,
            "plain.dsql",
            "query P {\n  title(limit 1) {\n    bogus\n  }\n}\n",
        )
        .await;
        insert_embedding_source(&bowl, "host.component", HOST, "typescript").await;

        let states = residency(&bowl).await;
        assert_eq!(
            states,
            vec![false, false, false],
            "plain document, host, and region are all evicted"
        );
        let before = diagnostic_entities(&bowl).await;
        assert_eq!(before.len(), 1, "the unknown column reports");

        // A second settle pass re-derives nothing: same fact entities,
        // still evicted, no residency flapping.
        let after = diagnostic_entities(&bowl).await;
        assert_eq!(before, after, "no re-derivation from eviction");
        assert_eq!(residency(&bowl).await, vec![false, false, false]);
    });
}

/// Rehydrating with identical content is fingerprint-neutral: facts keep
/// their entities. Different content re-derives.
#[test]
fn rehydration_rederives_only_on_real_changes() {
    block_on(async {
        let bowl = analysis_bowl().await;
        let source = "query P {\n  title(limit 1) {\n    bogus\n  }\n}\n";
        let file = insert_source(&bowl, "plain.dsql", source).await;
        let before = diagnostic_entities(&bowl).await;
        assert_eq!(before.len(), 1);

        let bowl = &bowl;
        let rehydrate = |text: String| async move {
            let rows = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
            for (entity, slot) in rows.collect() {
                if entity == file {
                    let text = text.clone();
                    slot.with_latest(move |slot| slot.set_text(&text)).await;
                }
            }
        };

        // Same content: same hash, no bump, diagnostics keep identity.
        rehydrate(source.to_string()).await;
        assert_eq!(
            diagnostic_entities(bowl).await,
            before,
            "same-content rehydration must not re-derive"
        );

        // Changed content: the diagnostic moves to the new name.
        rehydrate(source.replace("bogus", "fake")).await;
        let rows = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await;
        let messages: Vec<String> = rows
            .collect()
            .into_iter()
            .map(|(_, diagnostic)| diagnostic.0.clone())
            .collect();
        assert!(
            messages.iter().any(|message| message.contains("fake")),
            "changed content re-checks, got {messages:?}"
        );
    });
}

/// Open editor buffers never evict, even in analysis mode.
#[test]
fn open_buffers_stay_resident() {
    block_on(async {
        let bowl = analysis_bowl().await;
        let file = insert_source(
            &bowl,
            "open.dsql",
            "query O {\n  title(limit 1) {\n    id\n  }\n}\n",
        )
        .await;
        bowl.entity(file).insert((OpenBuffer,)).await;
        insert_source(
            &bowl,
            "closed.dsql",
            "query C {\n  kind_type {\n    kind\n  }\n}\n",
        )
        .await;

        let rows = bowl.scoop::<Query<(Entity, &SourceText)>>().await;
        for (entity, text) in rows.collect() {
            if entity == file {
                assert!(text.is_resident(), "open buffers keep their rope");
            } else {
                assert!(!text.is_resident(), "closed files evict");
            }
        }
    });
}

/// Eviction never changes the stored content hash.
#[test]
fn eviction_is_fingerprint_neutral() {
    let mut text = SourceText::from_text("query Q { title { id } }");
    let hash = text.content_hash();
    text.evict();
    assert!(!text.is_resident());
    assert_eq!(text.content_hash(), hash, "eviction keeps the hash");
    assert!(text.to_text().is_none());
    assert!(
        text.apply_edit(0..1, "x").is_err(),
        "incremental edits need a resident rope"
    );
    text.set_text("query Q { title { id } }");
    assert!(text.is_resident());
    assert_eq!(text.content_hash(), hash, "equal-content rehydration too");
}
