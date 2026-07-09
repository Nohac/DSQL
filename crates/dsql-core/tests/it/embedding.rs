//! Embedded-document extraction: host sources derive region entities that
//! parse, check, and render like plain files, re-derive on host edits,
//! keep their identity when their text is untouched, and reap when their
//! region vanishes.

use bowl::{Bowl, Entity, Mut, Query, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::facts::{PlanDemand, SqlDemand};
use dsql_core::register_language;
use dsql_core::source::{
    BelongsToHost, ResolutionScope, SourceOffset, SourceText, insert_host_source,
};
use dsql_core::sql::GeneratedSqlFact;
use futures::executor::block_on;

use crate::imdb_catalog;

const HOST: &str = r#"import { dsql } from "./dsql";

export const titles = dsql`
query Titles {
  title(limit 1) {
    id
  }
}
`;

export const kinds = dsql`
query Kinds {
  kind_type {
    kind
  }
}
`;
"#;

async fn host_bowl() -> (Bowl, Entity) {
    let bowl = Bowl::new();
    register_language(&bowl).await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let host = insert_host_source(
        &bowl,
        "src/queries.ts",
        HOST,
        ResolutionScope::default_scope(),
    )
    .await;
    (bowl, host)
}

async fn regions_of(bowl: &Bowl, host: Entity) -> Vec<(Entity, usize, String)> {
    let rows = bowl
        .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset, &SourceText)>>()
        .await;
    let mut regions: Vec<(Entity, usize, String)> = rows
        .collect()
        .into_iter()
        .filter(|(_, of, _, _)| of.0 == host)
        .map(|(entity, _, offset, text)| (entity, offset.0, text.to_text()))
        .collect();
    regions.sort_by_key(|(_, offset, _)| *offset);
    regions
}

#[test]
fn host_sources_derive_regions_that_compile() {
    block_on(async {
        let (bowl, host) = host_bowl().await;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
            .await;

        let regions = regions_of(&bowl, host).await;
        assert_eq!(regions.len(), 2, "one region per embedded template");
        for (_, offset, text) in &regions {
            assert_eq!(
                &HOST[*offset..offset + text.len()],
                text,
                "offsets point at the region inside the host"
            );
        }

        // Regions are documents: both embedded queries render SQL.
        let sql = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        assert_eq!(sql.len(), 2, "both embedded queries plan and render");
    });
}

#[test]
fn untouched_regions_keep_their_entities_across_host_edits() {
    block_on(async {
        let (bowl, host) = host_bowl().await;
        let before = regions_of(&bowl, host).await;

        // An edit outside every region: text and offsets of both regions
        // are unchanged, so re-extraction must be a no-op for them.
        let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (entity, source) in sources.collect() {
            if entity == host {
                source
                    .with_latest(|text| {
                        let appended = format!("{}\n// trailing comment\n", text.to_text());
                        text.set_text(&appended);
                    })
                    .await;
            }
        }

        let after = regions_of(&bowl, host).await;
        assert_eq!(after, before, "regions keep entities, offsets, and text");
    });
}

#[test]
fn vanished_regions_are_reaped() {
    block_on(async {
        let (bowl, host) = host_bowl().await;
        assert_eq!(regions_of(&bowl, host).await.len(), 2);

        let truncated = &HOST[..HOST.find("export const kinds").expect("fixture text")];
        let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (entity, source) in sources.collect() {
            if entity == host {
                let text_owned = truncated.to_string();
                source
                    .with_latest(move |text| text.set_text(&text_owned))
                    .await;
            }
        }

        let regions = regions_of(&bowl, host).await;
        assert_eq!(regions.len(), 1, "the removed region is reaped");
        assert!(regions[0].2.contains("query Titles"));
    });
}
