//! Embedded-document extraction: host sources derive region entities that
//! parse, check, and render like plain files, re-derive on host edits,
//! keep their identity when their text is untouched, and reap when their
//! region vanishes.

use std::collections::BTreeMap;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::embedding::{
    EmbeddedExpressionResolution, ExtractionRegistry, ExtractionStrategy,
    ResolvedEmbeddedExpression,
};
use dsql_core::entities::definition::{DefDecl, DefKind};
use dsql_core::facts::{DiagnosticsDemand, PlanDemand, SqlDemand, VariablesDemand};
use dsql_core::language_bowl;
use dsql_core::lint::LintConfig;
use dsql_core::source::{
    BelongsToHost, CallsiteSpan, SourceOffset, SourceText, insert_embedding_source,
};
use dsql_core::sql::GeneratedSqlFact;

use crate::{imdb_catalog, replace_source_text, set_source_text};

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

#[tokio::test]
async fn host_sources_derive_regions_that_compile() {
    let (bowl, host) = host_bowl().await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
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
}

#[tokio::test]
async fn named_extractors_coexist_and_ignore_host_extensions() {
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
}

#[tokio::test]
async fn untouched_regions_keep_their_entities_across_host_edits() {
    let (bowl, host) = host_bowl().await;
    let before = regions_of(&bowl, host).await;

    // An edit outside every region: text and offsets of both regions
    // are unchanged, so re-extraction must be a no-op for them.
    set_source_text(&bowl, host, format!("{HOST}\n// trailing comment\n")).await;

    let after = regions_of(&bowl, host).await;
    assert_eq!(after, before, "regions keep entities, offsets, and text");
}

#[tokio::test]
async fn moving_later_regions_preserves_their_semantics() {
    let (bowl, host) = host_bowl().await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    let before = regions_of(&bowl, host).await;

    replace_source_text(&bowl, host, "query Titles {", "query LongerTitles {").await;

    let after = regions_of(&bowl, host).await;
    assert_eq!(after.len(), before.len());
    for ((before_entity, before_offset, _), (after_entity, after_offset, _)) in
        before.iter().zip(&after)
    {
        assert_eq!(
            after_entity, before_entity,
            "region identity remains stable"
        );
        assert!(
            after_offset >= before_offset,
            "no region moves backwards after an insertion"
        );
    }
    assert_eq!(after[0].1, before[0].1, "the edited region starts in place");
    assert!(
        after[1..]
            .iter()
            .zip(&before[1..])
            .all(|((_, after_offset, _), (_, before_offset, _))| after_offset > before_offset),
        "every later region moves in host coordinates"
    );

    let resolutions = bowl.scoop::<Query<&ResolvedEmbeddedExpression>>().await;
    assert_eq!(
        resolutions
            .collect()
            .into_iter()
            .filter(|resolved| matches!(resolved.0, EmbeddedExpressionResolution::Target(_)))
            .count(),
        4,
        "every unchanged later expression remains resolved"
    );
    assert_eq!(
        crate::render_diagnostic_facts(&bowl).await,
        "",
        "moving regions does not invent expression-shape errors"
    );
}

/// Editing a host while diagnostics and lints are armed re-lowers clause
/// facts mid-settle; the ambient clause readers must sit behind the
/// Complete barrier or the same-phase race flag kills the process (the
/// "LSP crashes the second I edit" regression).
#[tokio::test]
async fn host_edits_with_armed_lints_do_not_race() {
    let (bowl, host) = host_bowl().await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl.insert((Singleton::<LintConfig>::new(), LintConfig::default()))
        .await;
    // Arm: settle once so lint rows exist before the edit.
    let _ = regions_of(&bowl, host).await;

    for round in 0..3 {
        replace_source_text(
            &bowl,
            host,
            "limit 1",
            if round % 2 == 0 { "limit 3" } else { "limit 1" },
        )
        .await;
        let regions = regions_of(&bowl, host).await;
        assert_eq!(regions.len(), 4, "regions survive armed edits");
    }
}

#[tokio::test]
async fn vanished_regions_are_reaped() {
    let (bowl, host) = host_bowl().await;
    assert_eq!(regions_of(&bowl, host).await.len(), 4);

    let truncated = &HOST[..HOST.find("export const kinds").expect("fixture text")];
    set_source_text(&bowl, host, truncated).await;

    let regions = regions_of(&bowl, host).await;
    assert_eq!(regions.len(), 1, "the removed regions are reaped");
    assert!(regions[0].2.contains("query Titles"));
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

    #[tokio::test]
    async fn hover_maps_host_positions_into_regions() {
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
    }

    #[tokio::test]
    async fn goto_definition_maps_host_positions() {
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
        assert!(
            matches!(target.as_ref(), DefinitionTarget::Source { .. }),
            "spread definition must target its source region"
        );
        if let DefinitionTarget::Source { file, span } = target.as_ref() {
            assert_eq!(*file, *bits_entity);
            assert_eq!(&bits_text[span.start..span.end], "TitleBits");
        }
    }

    #[tokio::test]
    async fn semantic_tokens_cover_host_files_in_host_coordinates() {
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
    }

    #[tokio::test]
    async fn completion_maps_host_positions_into_regions() {
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
    }
}

/// Every region records the span of its whole callsite expression — the
/// range a build-tool binding replaces — covering `dsql` through the
/// closing backtick/paren, in host coordinates.
#[tokio::test]
async fn regions_record_their_callsite_expressions() {
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
}

/// Every region resolves to its one definition, regardless of whether that
/// definition is a query or fragment.
#[tokio::test]
async fn embedded_expressions_resolve_query_and_fragment_targets() {
    let (bowl, _) = host_bowl().await;
    let resolutions = bowl.scoop::<Query<&ResolvedEmbeddedExpression>>().await;
    let definitions = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let definitions = definitions.collect();

    let mut targets = resolutions
        .collect()
        .into_iter()
        .filter_map(|resolved| match resolved.0 {
            EmbeddedExpressionResolution::Target(target) => definitions
                .iter()
                .find(|(entity, _)| *entity == target)
                .map(|(_, definition)| (definition.kind, definition.name.clone())),
            EmbeddedExpressionResolution::Empty
            | EmbeddedExpressionResolution::MultipleDefinitions(_) => None,
        })
        .collect::<Vec<_>>();
    targets.sort();

    assert_eq!(
        targets,
        vec![
            (DefKind::Query, "Kinds".to_string()),
            (DefKind::Query, "Panel".to_string()),
            (DefKind::Query, "Titles".to_string()),
            (DefKind::Fragment, "TitleBits".to_string()),
        ]
    );
}

/// The rewrite contract accepts exactly one query or fragment and rejects
/// empty expressions and every combination of multiple definitions.
#[tokio::test]
async fn embedded_expression_shapes_are_checked() {
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
    insert_embedding_source(
        &bowl,
        "src/multi-frags.ts",
        "export const c = dsql`\nfragment Id on title {\n  id\n}\nfragment Name on title {\n  title\n}\n`;\n",
        "typescript",
    )
    .await;
    insert_embedding_source(
        &bowl,
        "src/mixed.ts",
        "export const d = dsql`\nquery Titles {\n  title(limit 1) {\n    id\n  }\n}\nfragment MixedId on title {\n  id\n}\n`;\n",
        "typescript",
    )
    .await;
    insert_embedding_source(
        &bowl,
        "src/empty.ts",
        "export const e = dsql``;\n",
        "typescript",
    )
    .await;
    insert_embedding_source(
        &bowl,
        "src/broken.ts",
        "export const f = dsql`query`;\n",
        "typescript",
    )
    .await;

    insta::assert_snapshot!(crate::render_diagnostic_facts(&bowl).await);
}

/// The expression-shape check follows edits: a region growing a second
/// query after the first settle reports, shrinking back retires. Empty
/// expressions reject too — no rewrite target.
#[tokio::test]
async fn embedded_expression_shapes_follow_edits() {
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

    // Grow the first region to two queries.
    replace_source_text(
        &bowl,
        host,
        "query Titles {",
        "query Extra {\n  kind_type {\n    kind\n  }\n}\nquery Titles {",
    )
    .await;
    let reported = crate::render_diagnostic_facts(&bowl).await;
    assert!(
        reported.contains("defines 2 top-level definitions"),
        "the shape check follows the edit, got: {reported:?}"
    );

    // Shrink back: the diagnostic retires.
    replace_source_text(
        &bowl,
        host,
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
    replace_source_text(
        &bowl,
        host,
        "query Titles {\n  title(limit 1) {\n    id\n  }\n}",
        "",
    )
    .await;
    let reported = crate::render_diagnostic_facts(&bowl).await;
    assert!(
        reported.contains("defines no top-level definition"),
        "empty expressions reject, got: {reported:?}"
    );
}
