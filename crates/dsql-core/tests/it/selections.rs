//! Selection facts: field selections and fragment spreads lower into a flat
//! parent-keyed encoding of the selection tree, and spreads resolve to their
//! fragment definitions through the bound join.

use bowl::{Bowl, Entity, Mut, Query, Singleton};
use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::field_selection::FieldSel;
use dsql_core::entities::fragment_spread::{SpreadDecl, SpreadResolution};
use dsql_core::facts::{DiagnosticsDemand, NodeKey, ParentKey};
use dsql_core::register_language;
use dsql_core::source::{SourceText, insert_source};
use futures::executor::block_on;

use crate::{fixture, render_diagnostic_facts};

async fn language_bowl() -> Bowl {
    let bowl = Bowl::new();
    register_language(&bowl).await;
    bowl
}

/// Reconstructs the selection tree from the flat parent-keyed facts — the
/// same way planning and services consume them.
async fn render_selection_tree(bowl: &Bowl) -> String {
    let defs = bowl.scoop::<Query<(Entity, &DefDecl, &NodeKey)>>().await;
    let fields = bowl
        .scoop::<Query<(Entity, &FieldSel, &NodeKey, &ParentKey)>>()
        .await;
    let spreads = bowl
        .scoop::<Query<(Entity, &SpreadDecl, &NodeKey, &ParentKey)>>()
        .await;

    enum Node<'a> {
        Field(&'a FieldSel),
        Spread(&'a SpreadDecl),
    }

    let mut children: Vec<(NodeKey, NodeKey, usize, Node<'_>)> = Vec::new();
    let field_rows = fields.collect();
    for (_, field, key, parent) in &field_rows {
        children.push((parent.0, **key, field.span.start, Node::Field(field)));
    }
    let spread_rows = spreads.collect();
    for (_, spread, key, parent) in &spread_rows {
        children.push((parent.0, **key, spread.span.start, Node::Spread(spread)));
    }
    children.sort_by_key(|(_, _, start, _)| *start);

    fn render(
        parent: NodeKey,
        children: &[(NodeKey, NodeKey, usize, Node<'_>)],
        depth: usize,
        out: &mut String,
    ) {
        for (candidate_parent, key, _, node) in children {
            if *candidate_parent != parent {
                continue;
            }
            let indent = "  ".repeat(depth);
            match node {
                Node::Field(field) => {
                    let alias = field
                        .alias
                        .as_deref()
                        .map(|alias| format!("{alias}: "))
                        .unwrap_or_default();
                    let path = field
                        .relation_path
                        .as_deref()
                        .map(|path| format!("->{path}"))
                        .unwrap_or_default();
                    let nested = if field.nested { " {...}" } else { "" };
                    out.push_str(&format!("{indent}{alias}{}{path}{nested}\n", field.name));
                }
                Node::Spread(spread) => {
                    out.push_str(&format!("{indent}...{}\n", spread.name));
                }
            }
            render(*key, children, depth + 1, out);
        }
    }

    let mut def_rows = defs.collect();
    def_rows.sort_by_key(|(_, decl, _)| decl.span.start);
    let mut out = String::new();
    for (_, decl, key) in def_rows {
        out.push_str(&format!("{} {}\n", decl.kind, decl.name));
        render(*key, &children, 1, &mut out);
    }
    out
}

#[test]
fn selections_lower_into_a_parent_keyed_tree() {
    block_on(async {
        let bowl = language_bowl().await;

        insert_source(
            &bowl,
            "valid/imdb-relation-path-selector.dsql",
            &fixture("valid/imdb-relation-path-selector.dsql"),
        )
        .await;
        insert_source(
            &bowl,
            "valid/imdb-fragment-spread.dsql",
            &fixture("valid/imdb-fragment-spread.dsql"),
        )
        .await;

        insta::assert_snapshot!(render_selection_tree(&bowl).await);
    });
}

/// Renders spread resolutions by name, sorted for stability.
async fn render_resolutions(bowl: &Bowl) -> Vec<String> {
    let resolutions = bowl.scoop::<Query<(Entity, &SpreadResolution)>>().await;
    let spreads = bowl.scoop::<Query<(Entity, &SpreadDecl)>>().await;
    let defs = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let spread_rows = spreads.collect();
    let def_rows = defs.collect();

    let mut lines: Vec<String> = resolutions
        .collect()
        .into_iter()
        .map(|(_, resolution)| {
            let spread = spread_rows
                .iter()
                .find(|(entity, _)| *entity == resolution.spread)
                .map(|(_, decl)| decl.name.as_str())
                .unwrap_or("<missing spread>");
            let fragment = def_rows
                .iter()
                .find(|(entity, _)| *entity == resolution.fragment)
                .map(|(_, decl)| decl.name.as_str())
                .unwrap_or("<missing fragment>");
            format!("...{spread} -> fragment {fragment}")
        })
        .collect();
    lines.sort();
    lines
}

#[test]
fn spreads_resolve_to_fragments_in_the_same_file() {
    block_on(async {
        let bowl = language_bowl().await;

        insert_source(
            &bowl,
            "valid/imdb-fragment-spread.dsql",
            &fixture("valid/imdb-fragment-spread.dsql"),
        )
        .await;

        assert_eq!(
            render_resolutions(&bowl).await,
            vec!["...TitleFields -> fragment TitleFields"],
        );
    });
}

#[test]
fn renaming_a_fragment_retires_the_resolution() {
    block_on(async {
        let bowl = language_bowl().await;

        insert_source(
            &bowl,
            "rename.dsql",
            "fragment F on title {\n  id\n}\nquery Q {\n  title {\n    ...F\n  }\n}\n",
        )
        .await;
        assert_eq!(render_resolutions(&bowl).await.len(), 1);

        let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (_, source) in sources.collect() {
            source
                .with_latest(|text| {
                    text.set_text(
                        "fragment G on title {\n  id\n}\nquery Q {\n  title {\n    ...F\n  }\n}\n",
                    );
                })
                .await;
        }

        assert_eq!(
            render_resolutions(&bowl).await.len(),
            0,
            "resolution must retire with the renamed fragment"
        );

        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}
