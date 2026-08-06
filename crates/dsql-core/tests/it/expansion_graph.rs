//! Materialized fragment-expansion paths shared by semantic consumers.

use std::collections::HashMap;

use bowl::{Bowl, Entity, Query};
use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::expansion::{
    ExpansionCycle, ExpansionCycles, ExpansionOccurrence, ExpansionOccurrences,
};
use dsql_core::entities::fragment_spread::{ResolvedSpread, SpreadDecl};
use dsql_core::facts::SemanticRoot;
use dsql_core::source::insert_source;

use crate::replace_source_text;

async fn definition_labels(bowl: &Bowl) -> HashMap<Entity, String> {
    let definitions = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    definitions
        .collect()
        .into_iter()
        .map(|(entity, definition)| (entity, format!("{} {}", definition.kind, definition.name)))
        .collect()
}

async fn expansion_snapshot(bowl: &Bowl) -> String {
    let definition_labels = definition_labels(bowl).await;
    let occurrence_rows = bowl.scoop::<Query<(Entity, &ExpansionOccurrence)>>().await;
    let occurrences = occurrence_rows
        .collect()
        .into_iter()
        .map(|(entity, occurrence)| (entity, occurrence.clone()))
        .collect::<HashMap<_, _>>();
    drop(occurrence_rows);
    let cycle_rows = bowl.scoop::<Query<(Entity, &ExpansionCycle)>>().await;
    let cycles = cycle_rows
        .collect()
        .into_iter()
        .map(|(entity, cycle)| (entity, cycle.clone()))
        .collect::<HashMap<_, _>>();
    drop(cycle_rows);
    let groups = bowl.scoop::<Query<(Entity, &SemanticRoot)>>().await;
    let group_labels = groups
        .collect()
        .into_iter()
        .filter_map(|(group, root)| {
            definition_labels
                .get(&root.0)
                .cloned()
                .map(|label| (group, label))
        })
        .collect::<HashMap<_, _>>();
    drop(groups);

    let roots = bowl
        .scoop::<Query<(
            Entity,
            &SemanticRoot,
            Option<&ExpansionOccurrences>,
            Option<&ExpansionCycles>,
        )>>()
        .await;
    let mut lines = Vec::new();
    for (group, root, occurrence_ids, cycle_ids) in roots.collect() {
        let Some(root_label) = definition_labels.get(&root.0) else {
            continue;
        };
        for occurrence in occurrence_ids
            .into_iter()
            .flat_map(|ids| &ids.0)
            .map(|entity| {
                occurrences
                    .get(entity)
                    .expect("occurrence inverse names an occurrence")
            })
        {
            assert_eq!(occurrence.root_group, group);
            let target = group_labels
                .get(&occurrence.target_group)
                .expect("every occurrence targets a semantic group");
            let path = occurrence
                .path
                .iter()
                .map(|step| step.fragment.as_str())
                .collect::<Vec<_>>()
                .join(" > ");
            lines.push(format!("{root_label}: {path} -> {target}"));
        }
        for cycle in cycle_ids
            .into_iter()
            .flat_map(|ids| &ids.0)
            .map(|entity| cycles.get(entity).expect("cycle inverse names a cycle"))
        {
            assert_eq!(cycle.root_group, group);
            let path = cycle
                .path
                .iter()
                .map(|step| step.fragment.as_str())
                .collect::<Vec<_>>()
                .join(" > ");
            lines.push(format!("{root_label}: {path} -> cycle"));
        }
    }
    lines.sort();
    lines.join("\n")
}

async fn root_paths(bowl: &Bowl, wanted: &str) -> Vec<String> {
    expansion_snapshot(bowl)
        .await
        .lines()
        .filter_map(|line| {
            line.strip_prefix(wanted)
                .map(|path| path.trim_start_matches(": ").to_string())
        })
        .collect()
}

#[tokio::test]
async fn orphaned_spreads_stay_out_of_the_semantic_graph() {
    let bowl = dsql_core::language_bowl().await;
    insert_source(&bowl, "orphan.dsql", "query { title { ...Missing } }").await;

    assert_eq!(
        bowl.scoop::<Query<(Entity, &SpreadDecl)>>().await.len(),
        1,
        "error recovery still lowers the spread syntax"
    );
    assert_eq!(
        bowl.scoop::<Query<(Entity, &ResolvedSpread)>>().await.len(),
        0,
        "syntax without a lowered semantic root is not resolved"
    );
    assert_eq!(
        bowl.scoop::<Query<(Entity, &ExpansionOccurrence)>>()
            .await
            .len(),
        0,
        "orphaned syntax never enters expansion"
    );
}

#[tokio::test]
async fn expansion_paths_preserve_occurrences_and_cut_name_cycles() {
    let bowl = dsql_core::language_bowl().await;
    insert_source(
        &bowl,
        "expansion.dsql",
        indoc::indoc! {r#"
            fragment Leaf on title { id }
            fragment Left on title { ...Leaf }
            fragment Right on title { ...Leaf }
            fragment Diamond on title { ...Left ...Right }
            fragment Twice on title { ...Leaf ...Leaf }
            fragment Self on title { ...Self }
            fragment CycleA on title { ...CycleB }
            fragment CycleB on title { ...CycleA }
            query Root {
              title { ...Diamond ...Twice }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(expansion_snapshot(&bowl).await);
}

#[tokio::test]
async fn expansion_paths_rebuild_after_content_roundtrips() {
    const INITIAL: &str = indoc::indoc! {r#"
        fragment LeafA on title { id }
        fragment LeafB on title { title }
        fragment Switch on title { ...LeafA }
        query Root { title { ...Switch } }
    "#};
    const FINAL: &str = indoc::indoc! {r#"
        fragment LeafA on title { id }
        fragment LeafB on title { title }
        fragment Switch on title { ...LeafB }
        query Root { title { ...Switch } }
    "#};

    let incremental = dsql_core::language_bowl().await;
    let file = insert_source(&incremental, "roundtrip.dsql", INITIAL).await;
    let initial = root_paths(&incremental, "query Root").await;
    assert!(initial.iter().any(|path| path.contains("LeafA")));
    assert!(!initial.iter().any(|path| path.contains("LeafB")));

    replace_source_text(&incremental, file, "...LeafA", "...LeafB").await;
    let changed = root_paths(&incremental, "query Root").await;
    assert!(changed.iter().any(|path| path.contains("LeafB")));
    assert!(!changed.iter().any(|path| path.contains("LeafA")));

    replace_source_text(&incremental, file, "...LeafB", "...LeafA").await;
    let restored = root_paths(&incremental, "query Root").await;
    assert_eq!(restored, initial, "A-B-A retires and rebuilds descendants");

    let cold = dsql_core::language_bowl().await;
    insert_source(&cold, "roundtrip.dsql", INITIAL).await;
    assert_eq!(
        expansion_snapshot(&incremental).await,
        expansion_snapshot(&cold).await,
        "cold and incrementally restored semantic paths agree"
    );

    let final_bowl = dsql_core::language_bowl().await;
    insert_source(&final_bowl, "roundtrip.dsql", FINAL).await;
    assert_eq!(
        changed,
        root_paths(&final_bowl, "query Root").await,
        "cold and incremental changed states agree"
    );
}

#[tokio::test]
async fn expansion_depth_converges_without_a_fixed_walk_limit() {
    let mut source = String::from("fragment Level0 on title { id }\n");
    for depth in 1..=8 {
        source.push_str(&format!(
            "fragment Level{depth} on title {{ ...Level{} ...Level{} }}\n",
            depth - 1,
            depth - 1
        ));
    }
    source.push_str("query Root { title { ...Level8 } }\n");

    let bowl = dsql_core::language_bowl().await;
    insert_source(&bowl, "depth.dsql", &source).await;
    let paths = root_paths(&bowl, "query Root").await;
    assert_eq!(
        paths.len(),
        511,
        "the complete depth-eight path tree settles"
    );
    assert!(paths.iter().all(|path| !path.ends_with("cycle")));

    let explanation = bowl.explain("extend_expansion_occurrences").await;
    assert_eq!(explanation.matched_rows, 1515);
    assert_eq!(explanation.memoized_rows, explanation.matched_rows);
    assert_eq!(explanation.stale_views, 0);
}
