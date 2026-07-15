//! Embedded-document extraction: host sources derive region entities that
//! parse, check, and render like plain files, re-derive on host edits,
//! keep their identity when their text is untouched, and reap when their
//! region vanishes.

use std::collections::BTreeMap;

use bowl::{Bowl, Entity, Mut, Query, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::embedding::{ExtractionRegistry, ExtractionStrategy};
use dsql_core::facts::{DiagnosticsDemand, PlanDemand, SqlDemand};
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::{
    BelongsToHost, CallsiteSpan, SourceOffset, SourceText, insert_embedding_source,
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

export const bits = dsql`
fragment TitleBits on title {
  id
  title
}
`;

export const panel = dsql`
query Panel {
  title(limit 2) {
    ...TitleBits
    kind_type {
      kind
    }
  }
}
`;
"#;

async fn host_bowl() -> (Bowl, Entity) {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let host = insert_embedding_source(&bowl, "src/queries.host", HOST, "typescript").await;
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
        .map(|(entity, _, offset, text)| {
            (
                entity,
                offset.0,
                text.to_text().expect("scenario regions are resident"),
            )
        })
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
        assert_eq!(regions.len(), 4, "one region per embedded template");
        for (_, offset, text) in &regions {
            assert_eq!(
                &HOST[*offset..offset + text.len()],
                text,
                "offsets point at the region inside the host"
            );
        }

        // Regions are documents: every embedded query renders SQL.
        let sql = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        assert_eq!(sql.len(), 3, "all embedded queries plan and render");
    });
}

#[test]
fn named_extractors_coexist_and_ignore_host_extensions() {
    block_on(async {
        let bowl = language_bowl().await;
        bowl.insert((
            Singleton::<ExtractionRegistry>::new(),
            ExtractionRegistry(BTreeMap::from([
                (
                    "brackets".to_string(),
                    ExtractionStrategy::Regex {
                        pattern: r"QUERY\[(?P<content>[\s\S]*?)\]".to_string(),
                    },
                ),
                (
                    "angles".to_string(),
                    ExtractionStrategy::Regex {
                        pattern: r"DSQL<(?P<content>[\s\S]*?)>".to_string(),
                    },
                ),
            ])),
        ))
        .await;
        let bracket_host = insert_embedding_source(
            &bowl,
            "source.one",
            "QUERY[query Bracket { title { id } }] DSQL<ignored>",
            "brackets",
        )
        .await;
        let angle_host = insert_embedding_source(
            &bowl,
            "source.two",
            "QUERY[ignored] DSQL<query Angle { title { title } }>",
            "angles",
        )
        .await;

        let bracket_regions = regions_of(&bowl, bracket_host).await;
        let angle_regions = regions_of(&bowl, angle_host).await;
        assert_eq!(bracket_regions.len(), 1);
        assert_eq!(angle_regions.len(), 1);
        assert!(bracket_regions[0].2.contains("query Bracket"));
        assert!(angle_regions[0].2.contains("query Angle"));
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
                        let appended = format!(
                            "{}\n// trailing comment\n",
                            text.to_text().expect("editor text is resident")
                        );
                        text.set_text(&appended);
                    })
                    .await;
            }
        }

        let after = regions_of(&bowl, host).await;
        assert_eq!(after, before, "regions keep entities, offsets, and text");
    });
}

/// Editing a host while diagnostics and lints are armed re-lowers clause
/// facts mid-settle; the ambient clause readers must sit behind the
/// Complete barrier or the same-phase race flag kills the process (the
/// "LSP crashes the second I edit" regression).
#[test]
fn host_edits_with_armed_lints_do_not_race() {
    block_on(async {
        let (bowl, host) = host_bowl().await;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<LintConfig>::new(), LintConfig::default()))
            .await;
        // Arm: settle once so lint rows exist before the edit.
        let _ = regions_of(&bowl, host).await;

        for round in 0..3 {
            let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
            for (entity, source) in sources.collect() {
                if entity == host {
                    source
                        .with_latest(move |text| {
                            let edited = text.to_text().expect("editor text is resident").replace(
                                "limit 1",
                                if round % 2 == 0 { "limit 3" } else { "limit 1" },
                            );
                            text.set_text(&edited);
                        })
                        .await;
                }
            }
            let regions = regions_of(&bowl, host).await;
            assert_eq!(regions.len(), 4, "regions survive armed edits");
        }
    });
}

#[test]
fn vanished_regions_are_reaped() {
    block_on(async {
        let (bowl, host) = host_bowl().await;
        assert_eq!(regions_of(&bowl, host).await.len(), 4);

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
        assert_eq!(regions.len(), 1, "the removed regions are reaped");
        assert!(regions[0].2.contains("query Titles"));
    });
}

/// Editor requests arrive with the host path and host-coordinate offsets;
/// enrichment rebases them onto the containing region. This is the
/// regression suite for "hover/goto/tokens do nothing in .tsx files".
mod host_requests {
    use dsql_core::service::{CompletionList, CompletionRequest};
    use dsql_core::service::{
        DefinitionRequest, DefinitionTarget, HoverInfo, HoverRequest, Position, priority,
        semantic_tokens,
    };
    use dsql_core::source::FilePath;

    use super::*;

    async fn hover_at(bowl: &Bowl, offset: usize) -> HoverInfo {
        let info = bowl
            .insert((
                HoverRequest,
                FilePath("src/queries.host".to_string()),
                Position { offset },
            ))
            .await
            .bind()
            .take::<HoverInfo>()
            .await
            .expect("hover answered");
        HoverInfo {
            text: info.text.clone(),
            priority: info.priority,
        }
    }

    #[test]
    fn hover_maps_host_positions_into_regions() {
        block_on(async {
            let (bowl, _) = host_bowl().await;

            // A column inside the first template.
            let id = HOST.find("    id").expect("fixture text") + 4;
            assert_eq!(
                hover_at(&bowl, id).await.text,
                "column `id`: int (not null)"
            );

            // The fragment definition's name, in the third template.
            let name = HOST.find("fragment TitleBits").expect("fixture text") + "fragment ".len();
            assert_eq!(
                hover_at(&bowl, name).await.text,
                "fragment `TitleBits` on `title`"
            );

            // The spread in the fourth template.
            let spread = HOST.find("...TitleBits").expect("fixture text") + "...".len();
            assert_eq!(
                hover_at(&bowl, spread).await.text,
                "fragment `TitleBits` on `title`"
            );

            // TypeScript code between regions answers nothing.
            let import = HOST.find("import").expect("fixture text");
            assert!(hover_at(&bowl, import).await.priority <= priority::RESOLVED);
        });
    }

    #[test]
    fn goto_definition_maps_host_positions() {
        block_on(async {
            let (bowl, host) = host_bowl().await;

            let spread = HOST.find("...TitleBits").expect("fixture text") + "...".len();
            let target = bowl
                .insert((
                    DefinitionRequest,
                    FilePath("src/queries.host".to_string()),
                    Position { offset: spread },
                ))
                .await
                .bind()
                .take::<DefinitionTarget>()
                .await
                .expect("definition answered");

            // The target is the region entity holding the fragment, with a
            // region-relative span; the LSP boundary shifts it back to host
            // coordinates through BelongsToHost + SourceOffset.
            let regions = regions_of(&bowl, host).await;
            let (bits_entity, _, bits_text) = regions
                .iter()
                .find(|(_, _, text)| text.contains("fragment TitleBits"))
                .expect("fragment region derived");
            assert_eq!(target.file, *bits_entity);
            assert_eq!(&bits_text[target.span.start..target.span.end], "TitleBits");
        });
    }

    #[test]
    fn semantic_tokens_cover_host_files_in_host_coordinates() {
        block_on(async {
            let (bowl, _) = host_bowl().await;

            let tokens = semantic_tokens(&bowl, "src/queries.host").await;
            assert!(!tokens.is_empty(), "host files highlight their regions");

            let slice = |span: dsql_core::facts::Span| &HOST[span.start..span.end];
            let texts: Vec<(String, String)> = tokens
                .iter()
                .map(|token| (format!("{:?}", token.kind), slice(token.span).to_string()))
                .collect();
            assert!(texts.contains(&("Fragment".into(), "TitleBits".into())));
            assert!(texts.contains(&("Table".into(), "title".into())));
            assert!(texts.contains(&("Column".into(), "kind".into())));

            // Spans are host coordinates: sorted and within the host text.
            assert!(
                tokens
                    .windows(2)
                    .all(|w| w[0].span.start <= w[1].span.start)
            );
            assert!(tokens.iter().all(|token| token.span.end <= HOST.len()));
        });
    }

    #[test]
    fn completion_maps_host_positions_into_regions() {
        block_on(async {
            let (bowl, _) = host_bowl().await;

            // Cursor right after `...` in the panel template: fragment
            // candidates apply.
            let offset = HOST.find("...TitleBits").expect("fixture text") + "...".len();
            let list = bowl
                .insert((
                    CompletionRequest,
                    FilePath("src/queries.host".to_string()),
                    Position { offset },
                ))
                .await
                .bind()
                .take::<CompletionList>()
                .await
                .expect("completion answered");
            assert!(
                list.items.iter().any(|item| item.label == "TitleBits"),
                "expected TitleBits candidate, got: {:?}",
                list.items
                    .iter()
                    .map(|item| &item.label)
                    .collect::<Vec<_>>()
            );
        });
    }
}

/// Every region records the span of its whole callsite expression — the
/// range a build-tool binding replaces — covering `dsql` through the
/// closing backtick/paren, in host coordinates.
#[test]
fn regions_record_their_callsite_expressions() {
    block_on(async {
        let (bowl, host) = host_bowl().await;
        let rows = bowl
            .scoop::<Query<(Entity, &BelongsToHost, &CallsiteSpan, &SourceOffset)>>()
            .await;
        let mut callsites: Vec<(usize, usize, usize)> = rows
            .collect()
            .into_iter()
            .filter(|(_, of, _, _)| of.0 == host)
            .map(|(_, _, callsite, offset)| (callsite.0.start, callsite.0.end, offset.0))
            .collect();
        callsites.sort();
        assert_eq!(callsites.len(), 4, "one callsite per region");
        for (start, end, content_start) in callsites {
            let expression = &HOST[start..end];
            assert!(
                expression.starts_with("dsql`") && expression.ends_with('`'),
                "the span covers the whole expression, got {expression:?}"
            );
            assert!(
                start < content_start && content_start < end,
                "the content sits inside the expression"
            );
        }
    });
}

/// The rewrite contract rejects embedded expressions that define several
/// queries or only fragments; exactly-one-query (with helper fragments in
/// .dsql files) stays clean.
#[test]
fn embedded_expression_shapes_are_checked() {
    block_on(async {
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        insert_embedding_source(
            &bowl,
            "src/multi.ts",
            "export const a = dsql`\nquery One {\n  title(limit 1) {\n    id\n  }\n}\nquery Two {\n  kind_type {\n    kind\n  }\n}\n`;\n",
            "typescript",
        )
        .await;
        insert_embedding_source(
            &bowl,
            "src/frags.ts",
            "export const b = dsql`\nfragment Bits on title {\n  id\n}\n`;\n",
            "typescript",
        )
        .await;

        insta::assert_snapshot!(crate::render_diagnostic_facts(&bowl).await);
    });
}

/// The expression-shape check follows edits: a region growing a second
/// query after the first settle reports, shrinking back retires. Empty
/// expressions reject too — no rewrite target.
#[test]
fn embedded_expression_shapes_follow_edits() {
    block_on(async {
        // A host of its own: the shared fixture contains a deliberate
        // fragment-only region, which this check rejects by design.
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        let host = insert_embedding_source(
            &bowl,
            "src/single.ts",
            "export const q = dsql`\nquery Titles {\n  title(limit 1) {\n    id\n  }\n}\n`;\n",
            "typescript",
        )
        .await;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        assert_eq!(
            crate::render_diagnostic_facts(&bowl).await,
            "",
            "the fixture host starts clean"
        );

        let edit = |from: &'static str, to: &'static str| {
            let bowl = &bowl;
            async move {
                let rows = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
                for (entity, source) in rows.collect() {
                    if entity == host {
                        source
                            .with_latest(move |text| {
                                let edited = text
                                    .to_text()
                                    .expect("editor text is resident")
                                    .replace(from, to);
                                text.set_text(&edited);
                            })
                            .await;
                    }
                }
            }
        };

        // Grow the first region to two queries.
        edit(
            "query Titles {",
            "query Extra {\n  kind_type {\n    kind\n  }\n}\nquery Titles {",
        )
        .await;
        let reported = crate::render_diagnostic_facts(&bowl).await;
        assert!(
            reported.contains("defines 2 queries"),
            "the shape check follows the edit, got: {reported:?}"
        );

        // Shrink back: the diagnostic retires.
        edit(
            "query Extra {\n  kind_type {\n    kind\n  }\n}\nquery Titles {",
            "query Titles {",
        )
        .await;
        assert_eq!(
            crate::render_diagnostic_facts(&bowl).await,
            "",
            "shrinking back retires the diagnostic"
        );

        // Empty out the first region entirely: no rewrite target.
        edit("query Titles {\n  title(limit 1) {\n    id\n  }\n}", "").await;
        let reported = crate::render_diagnostic_facts(&bowl).await;
        assert!(
            reported.contains("defines no query"),
            "empty expressions reject, got: {reported:?}"
        );
    });
}
