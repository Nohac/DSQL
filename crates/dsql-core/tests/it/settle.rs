//! Settled-bowl snapshots: files enter the bowl, parse into CSTs, and parse
//! errors surface (and clean up) as derived diagnostic facts.

use bowl::{Bowl, Entity, Query, With};
use dsql_core::entities::document::ParsedFile;
use dsql_core::facts::Diagnostic;
use dsql_core::source::{FilePath, insert_source};

use crate::{fixture, render_diagnostic_facts, set_source_text};

async fn language_bowl() -> Bowl {
    dsql_core::language_bowl().await
}

#[tokio::test]
async fn files_parse_into_csts() {
    let bowl = language_bowl().await;

    insert_source(
        &bowl,
        "valid/imdb-title-basic.dsql",
        &fixture("valid/imdb-title-basic.dsql"),
    )
    .await;
    insert_source(
        &bowl,
        "valid/imdb-movie-info-basic.dsql",
        &fixture("valid/imdb-movie-info-basic.dsql"),
    )
    .await;

    let parsed = bowl
        .scoop::<Query<(Entity, &FilePath), With<ParsedFile>>>()
        .await;
    let mut paths: Vec<String> = parsed
        .collect()
        .into_iter()
        .map(|(_, path)| path.0.clone())
        .collect();
    paths.sort();

    let diagnostics = render_diagnostic_facts(&bowl).await;
    insta::assert_snapshot!(format!(
        "parsed files:\n{}\ndiagnostics:\n{diagnostics}",
        paths.join("\n"),
    ));
}

#[tokio::test]
async fn parse_errors_become_diagnostic_facts() {
    let bowl = language_bowl().await;

    insert_source(&bowl, "broken.dsql", "query Broken {\n  title(where\n}\n").await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn fixing_a_file_cleans_stale_diagnostics() {
    let bowl = language_bowl().await;

    let file = insert_source(&bowl, "fixme.dsql", "query Broken {\n  title(where\n}\n").await;

    let before = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
    assert!(before > 0, "the broken file must produce parse diagnostics");

    set_source_text(&bowl, file, "query Fixed {\n  title {\n    id\n  }\n}\n").await;

    let after = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
    assert_eq!(after, 0, "stale parse diagnostics must clean up on edit");
}
