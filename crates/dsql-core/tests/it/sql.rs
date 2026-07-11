//! Plan and SQL generation: fixture queries plan and render to PostgreSQL
//! on demand.

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{PlanDemand, SqlDemand};
use dsql_core::language_bowl;
use dsql_core::source::insert_source;
use dsql_core::sql::{GeneratedSqlFact, SqlOptions};
use futures::executor::block_on;

use crate::{fixture, imdb_catalog};

async fn sql_bowl(catalog: Catalog) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
        .await;
    bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
        .await;
    // Bound nested collections at 10, the reference snapshot setting.
    bowl.insert((
        Singleton::<SqlOptions>::new(),
        SqlOptions {
            collection_limit: Some(10),
        },
    ))
    .await;
    bowl
}

/// Renders all generated SQL facts, one section per query.
async fn render_sql(bowl: &Bowl) -> String {
    let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let mut generated: Vec<&GeneratedSqlFact> =
        rows.collect().into_iter().map(|(_, fact)| fact).collect();
    generated.sort_by(|left, right| left.0.output_name.cmp(&right.0.output_name));
    generated
        .into_iter()
        .map(|fact| {
            let mut section = format!("-- {}\n{}", fact.0.output_name, fact.0.sql);
            if !fact.0.parameters.is_empty() {
                let parameters: Vec<&str> = fact
                    .0
                    .parameters
                    .iter()
                    .map(|parameter| parameter.path.as_str())
                    .collect();
                section.push_str(&format!("\n-- parameters: {}", parameters.join(", ")));
            }
            if !fact.0.variants.is_empty() {
                for variant in &fact.0.variants {
                    let cases: Vec<String> = variant
                        .cases
                        .iter()
                        .map(|case| format!("{}=>{}", case.value, case.text))
                        .collect();
                    section.push_str(&format!(
                        "\n-- variant {}: {}",
                        variant.path,
                        cases.join(", ")
                    ));
                }
            }
            section
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn fixture_sql(name: &str) -> String {
    let bowl = sql_bowl(imdb_catalog()).await;
    insert_source(&bowl, name, &fixture(name)).await;
    render_sql(&bowl).await
}

#[test]
fn title_basic_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-title-basic.dsql").await);
    });
}

#[test]
fn movie_info_basic_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-movie-info-basic.dsql").await);
    });
}

#[test]
fn relation_path_selector_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-relation-path-selector.dsql").await);
    });
}

#[test]
fn scoped_relation_predicate_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-scoped-relation-predicate.dsql").await);
    });
}

#[test]
fn fragment_spread_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-fragment-spread.dsql").await);
    });
}

#[test]
fn variables_render_as_parameters_and_variants() {
    block_on(async {
        let bowl = sql_bowl(Catalog::hardcoded()).await;
        insert_source(
            &bowl,
            "params.dsql",
            "query UsersPage {\n  users(\n    where .name $$name_op[==, like] $$name and .id == $tenant\n    order by created_at $$dir\n    limit $$max\n    offset $skip\n  ) {\n    id\n    posts(limit 5) {\n      title\n    }\n  }\n}\n",
        )
        .await;

        insta::assert_snapshot!(render_sql(&bowl).await);
    });
}

#[test]
fn plans_retire_when_demand_is_removed_sources_change() {
    block_on(async {
        let bowl = sql_bowl(imdb_catalog()).await;
        insert_source(&bowl, "q.dsql", "query Q {\n  title {\n    id\n  }\n}\n").await;

        let generated = bowl
            .scoop::<Query<(Entity, &GeneratedSqlFact)>>()
            .await
            .len();
        assert_eq!(generated, 1);

        let sources = bowl
            .scoop::<Query<(Entity, bowl::Mut<dsql_core::source::SourceText>)>>()
            .await;
        for (_, source) in sources.collect() {
            source
                .with_latest(|text| text.set_text("query Q {\n  kind_type {\n    kind\n  }\n}\n"))
                .await;
        }

        let rows = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
        let names: Vec<String> = rows
            .collect()
            .into_iter()
            .map(|(_, fact)| fact.0.output_name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["kind_type".to_string()],
            "stale SQL must retire with the edit"
        );
    });
}

/// Comparison right-hand sides that are themselves column paths: same-table
/// columns render as plain column references, and relation-path RHS columns
/// exercise the `OuterCurrent` scope inside the nested `EXISTS`.
#[test]
fn rhs_same_table_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-rhs-same-table.dsql").await);
    });
}

#[test]
fn rhs_relation_path_sql() {
    block_on(async {
        insta::assert_snapshot!(fixture_sql("valid/imdb-rhs-relation-path.dsql").await);
    });
}
/// Fragment bodies are expanded by walks in *other* files; the DefIndex
/// fragment fingerprint is the tracked dependency that keeps dependents
/// fresh across files.
#[test]
fn cross_file_fragment_body_edits_rederive_sql() {
    use bowl::Mut;
    use dsql_core::source::SourceText;
    block_on(async {
        let bowl = sql_bowl(imdb_catalog()).await;
        insert_source(&bowl, "frag.dsql", "fragment Bits on title {\n  id\n}\n").await;
        insert_source(
            &bowl,
            "query.dsql",
            "query UsesBits {\n  title(limit 1) {\n    ...Bits\n  }\n}\n",
        )
        .await;
        let before = render_sql(&bowl).await;
        assert!(
            before.contains("\"id\""),
            "fragment field planned: {before}"
        );

        let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (_, source) in sources.collect() {
            source
                .with_latest(|text| {
                    if text.to_text().contains("fragment Bits") {
                        text.set_text("fragment Bits on title {\n  title\n}\n");
                    }
                })
                .await;
        }
        let after = render_sql(&bowl).await;
        // Snapshot the whole post-edit render: a false pass from dropped
        // expansion (rather than re-derived expansion) is impossible to
        // write through a full snapshot.
        insta::assert_snapshot!(after);
    });
}
