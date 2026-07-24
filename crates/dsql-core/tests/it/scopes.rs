//! Resolution scopes: fragments resolve across files within a scope and
//! through imports; independent scopes stay independent; collisions and
//! ambiguities are diagnostics (docs/spec/resolution-scopes.md).

use std::collections::BTreeMap;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::entities::fragment_spread::ResolvedSpread;
use dsql_core::facts::DiagnosticsDemand;
use dsql_core::language_bowl;
use dsql_core::source::{
    ResolutionScope, ScopeDocument, ScopeDocuments, ScopeImports, ScopeOwnership, SourceAssignment,
    SourceKind, insert_source_scoped,
};

use crate::{imdb_catalog, render_diagnostic_facts, set_source_text};

async fn scoped_bowl(imports: &[(&str, &[&str])]) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let imports: BTreeMap<String, Vec<String>> = imports
        .iter()
        .map(|(scope, imported)| {
            (
                (*scope).to_string(),
                imported
                    .iter()
                    .map(|import| (*import).to_string())
                    .collect(),
            )
        })
        .collect();
    bowl.insert((Singleton::<ScopeImports>::new(), ScopeImports(imports)))
        .await;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl
}

async fn insert(bowl: &Bowl, path: &str, scope: &str, text: &str) {
    insert_source_scoped(
        bowl,
        path,
        text,
        ResolutionScope(scope.to_string()),
        dsql_core::source::SourceKind::Dsql,
    )
    .await;
}

async fn resolutions(bowl: &Bowl) -> usize {
    let spreads = bowl.scoop::<Query<(Entity, &ResolvedSpread)>>().await;
    spreads
        .collect()
        .into_iter()
        .filter(|(_, resolved)| resolved.target.is_some())
        .count()
}

const FRAGMENT: &str = "fragment TitleBits on title {\n  id\n}\n";
const SPREAD: &str = "query Q {\n  title {\n    ...TitleBits\n  }\n}\n";

#[tokio::test]
async fn fragments_resolve_across_files_within_a_scope() {
    let bowl = scoped_bowl(&[]).await;
    insert(&bowl, "fragments.dsql", "default", FRAGMENT).await;
    insert(&bowl, "query.dsql", "default", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 1);
    assert_eq!(render_diagnostic_facts(&bowl).await, "");
}

#[tokio::test]
async fn imported_scopes_provide_fragments() {
    let bowl = scoped_bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
    insert(&bowl, "shared.dsql", "shared", FRAGMENT).await;
    insert(&bowl, "page.dsql", "frontend", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 1);
    assert_eq!(render_diagnostic_facts(&bowl).await, "");
}

#[tokio::test]
async fn transitive_imports_provide_fragments() {
    let bowl = scoped_bowl(&[
        ("frontend", &["middle"]),
        ("middle", &["shared"]),
        ("shared", &[]),
    ])
    .await;
    insert(&bowl, "shared.dsql", "shared", FRAGMENT).await;
    insert(&bowl, "page.dsql", "frontend", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 1);
    assert_eq!(render_diagnostic_facts(&bowl).await, "");
}

#[tokio::test]
async fn diamond_imports_do_not_duplicate_one_origin() {
    let bowl = scoped_bowl(&[
        ("frontend", &["left", "right"]),
        ("left", &["shared"]),
        ("right", &["shared"]),
        ("shared", &[]),
    ])
    .await;
    insert(&bowl, "shared.dsql", "shared", FRAGMENT).await;
    insert(&bowl, "page.dsql", "frontend", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 1, "one origin stays unique");
    assert_eq!(render_diagnostic_facts(&bowl).await, "");
}

#[test]
fn cyclic_import_visibility_is_finite_and_deduplicated() {
    let imports = ScopeImports(BTreeMap::from([
        ("a".to_string(), vec!["b".to_string()]),
        ("b".to_string(), vec!["c".to_string()]),
        ("c".to_string(), vec!["a".to_string()]),
    ]));

    assert_eq!(
        imports.visible_from("a").collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
}

#[test]
fn terminal_generation_targets_come_from_the_complete_scope_graph() {
    let imports = ScopeImports(BTreeMap::from([
        ("api".to_string(), vec!["shared".to_string()]),
        ("analytics".to_string(), vec!["shared".to_string()]),
        ("shared".to_string(), Vec::new()),
        ("shared_output".to_string(), vec!["shared".to_string()]),
    ]));

    assert_eq!(
        imports.generation_targets().collect::<Vec<_>>(),
        ["analytics", "api", "shared_output"]
    );
    assert!(imports.is_generation_target("api"));
    assert!(!imports.is_generation_target("shared"));
    assert!(!imports.is_generation_target("missing"));
}

#[tokio::test]
async fn scopes_without_imports_do_not_see_each_other() {
    let bowl = scoped_bowl(&[("api", &[]), ("frontend", &[])]).await;
    insert(&bowl, "api.dsql", "api", FRAGMENT).await;
    insert(&bowl, "page.dsql", "frontend", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 0);
    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn same_name_in_independent_scopes_is_clean() {
    let bowl = scoped_bowl(&[("api", &[]), ("frontend", &[])]).await;
    insert(&bowl, "api.dsql", "api", FRAGMENT).await;
    insert(&bowl, "api-query.dsql", "api", SPREAD).await;
    insert(&bowl, "page-fragments.dsql", "frontend", FRAGMENT).await;
    insert(&bowl, "page.dsql", "frontend", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 2, "each scope resolves locally");
    assert_eq!(render_diagnostic_facts(&bowl).await, "");
}

#[tokio::test]
async fn local_fragment_colliding_with_import_is_reported() {
    let bowl = scoped_bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
    insert(&bowl, "shared.dsql", "shared", FRAGMENT).await;
    insert(&bowl, "page-fragments.dsql", "frontend", FRAGMENT).await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn local_collision_names_the_lexicographically_first_provider() {
    let bowl = scoped_bowl(&[("frontend", &["b", "a"]), ("a", &[]), ("b", &[])]).await;
    insert(&bowl, "a.dsql", "a", FRAGMENT).await;
    insert(&bowl, "b.dsql", "b", FRAGMENT).await;
    insert(&bowl, "page-fragments.dsql", "frontend", FRAGMENT).await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn fragment_provided_by_two_imports_is_ambiguous_at_the_spread() {
    let bowl = scoped_bowl(&[("frontend", &["a", "b"]), ("a", &[]), ("b", &[])]).await;
    insert(&bowl, "a.dsql", "a", FRAGMENT).await;
    insert(&bowl, "b.dsql", "b", FRAGMENT).await;
    insert(&bowl, "page.dsql", "frontend", SPREAD).await;

    assert_eq!(resolutions(&bowl).await, 0);
    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

const QUERY: &str = "query Titles {\n  title(limit 1) {\n    id\n  }\n}\n";

/// A local query colliding with an imported one is a language error at
/// the local definition — mirrors the fragment rule.
#[tokio::test]
async fn local_query_colliding_with_import_is_reported() {
    let bowl = scoped_bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
    insert(&bowl, "shared.dsql", "shared", QUERY).await;
    insert(&bowl, "page.dsql", "frontend", QUERY).await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn local_query_colliding_with_transitive_import_is_reported() {
    let bowl = scoped_bowl(&[
        ("frontend", &["middle"]),
        ("middle", &["shared"]),
        ("shared", &[]),
    ])
    .await;
    insert(&bowl, "shared.dsql", "shared", QUERY).await;
    insert(&bowl, "page.dsql", "frontend", QUERY).await;

    let diagnostics = render_diagnostic_facts(&bowl).await;
    assert!(
        diagnostics.contains("imported from scope `shared`"),
        "transitive imports participate in collisions: {diagnostics}"
    );
    insta::assert_snapshot!(diagnostics);
}

/// Two imported scopes providing one query name to a consuming scope
/// collide in its artifact closure even with no local definition or use
/// site — reported once, on the first provider, naming every provider.
/// (Fragments keep their spread-site ambiguity diagnostic instead.)
#[tokio::test]
async fn query_provided_by_two_imports_collides_at_the_definition() {
    let bowl = scoped_bowl(&[("frontend", &["a", "b"]), ("a", &[]), ("b", &[])]).await;
    insert(&bowl, "a.dsql", "a", QUERY).await;
    insert(&bowl, "b.dsql", "b", QUERY).await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

/// The import-ambiguity check follows edits: introducing the second
/// provider after the first settle reports, renaming it away retires.
#[tokio::test]
async fn import_ambiguities_follow_edits() {
    let bowl = scoped_bowl(&[("frontend", &["a", "b"]), ("a", &[]), ("b", &[])]).await;
    insert(&bowl, "a.dsql", "a", QUERY).await;
    insert(
        &bowl,
        "b.dsql",
        "b",
        "query Other {\n  title(limit 1) {\n    id\n  }\n}\n",
    )
    .await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "distinct names are clean"
    );

    // Rename b's query onto a's name: the ambiguity appears.
    let target = {
        let rows = bowl
            .scoop::<Query<(Entity, &dsql_core::source::FilePath)>>()
            .await;
        rows.collect()
            .into_iter()
            .find(|(_, path)| path.0 == "b.dsql")
            .map(|(entity, _)| entity)
            .expect("b.dsql exists")
    };
    set_source_text(&bowl, target, QUERY).await;
    let reported = render_diagnostic_facts(&bowl).await;
    assert!(
        reported.contains("provided to scope `frontend`"),
        "the ambiguity appears after the edit, got: {reported:?}"
    );

    set_source_text(
        &bowl,
        target,
        "query Other {\n  title(limit 1) {\n    id\n  }\n}\n",
    )
    .await;
    assert_eq!(
        render_diagnostic_facts(&bowl).await,
        "",
        "renaming away retires the ambiguity"
    );
}

/// Scope ownership preserves its outcomes: unmatched and overlapping
/// paths must not silently collapse into the default scope.
#[test]
fn scope_ownership_distinguishes_outcomes() {
    assert_eq!(SourceKind::from_resolver("dsql"), SourceKind::Dsql);
    assert_eq!(
        SourceKind::from_resolver("custom"),
        SourceKind::Embedded("custom".to_string())
    );

    let empty = ScopeDocuments::default();
    assert_eq!(
        empty.ownership_of("/p/queries/a.dsql"),
        ScopeOwnership::ImplicitDefault
    );

    let configured = ScopeDocuments(vec![
        (
            "shared".to_string(),
            vec![ScopeDocument {
                kind: SourceKind::Dsql,
                paths: vec!["/p/queries/shared/**/*.dsql".to_string()],
            }],
        ),
        (
            "frontend".to_string(),
            vec![ScopeDocument {
                kind: SourceKind::Dsql,
                paths: vec![
                    "/p/queries/frontend/**/*.dsql".to_string(),
                    "/p/queries/shared/both.dsql".to_string(),
                ],
            }],
        ),
    ]);
    assert_eq!(
        configured.ownership_of("/p/queries/frontend/new.dsql"),
        ScopeOwnership::Unique(SourceAssignment {
            scope: "frontend".to_string(),
            kind: SourceKind::Dsql,
        })
    );
    assert_eq!(
        configured.ownership_of("/p/other/loose.dsql"),
        ScopeOwnership::Unmatched
    );
    assert_eq!(
        configured.ownership_of("/p/queries/shared/both.dsql"),
        ScopeOwnership::Ambiguous(vec![
            SourceAssignment {
                scope: "frontend".to_string(),
                kind: SourceKind::Dsql,
            },
            SourceAssignment {
                scope: "shared".to_string(),
                kind: SourceKind::Dsql,
            },
        ])
    );

    let broad = ScopeDocuments(vec![(
        "broad".to_string(),
        vec![ScopeDocument {
            kind: SourceKind::Embedded("custom".to_string()),
            paths: vec![
                "/p/sources".to_string(),
                "/p/glob/**/*".to_string(),
                "/p/exact/readme.md".to_string(),
            ],
        }],
    )]);
    assert_eq!(
        broad.ownership_of("/p/sources/readme.md"),
        ScopeOwnership::Unique(SourceAssignment {
            scope: "broad".to_string(),
            kind: SourceKind::Embedded("custom".to_string()),
        })
    );
    assert_eq!(
        broad.ownership_of("/p/glob/nested/component.vue"),
        ScopeOwnership::Unique(SourceAssignment {
            scope: "broad".to_string(),
            kind: SourceKind::Embedded("custom".to_string()),
        })
    );
    assert_eq!(
        ScopeDocuments(vec![(
            "default".to_string(),
            vec![ScopeDocument {
                kind: SourceKind::Dsql,
                paths: vec!["/p/dsql/**/*.dsql".to_string()],
            }],
        )])
        .ownership_of("/p/dsql/root.dsql"),
        ScopeOwnership::Unique(SourceAssignment {
            scope: "default".to_string(),
            kind: SourceKind::Dsql,
        })
    );

    let flat = ScopeDocuments(vec![(
        "flat".to_string(),
        vec![ScopeDocument {
            kind: SourceKind::Dsql,
            paths: vec!["/p/queries/*.dsql".to_string()],
        }],
    )]);
    assert_eq!(
        flat.ownership_of("/p/queries/direct.dsql"),
        ScopeOwnership::Unique(SourceAssignment {
            scope: "flat".to_string(),
            kind: SourceKind::Dsql,
        })
    );
    assert_eq!(
        flat.ownership_of("/p/queries/nested/deep.dsql"),
        ScopeOwnership::Unmatched
    );
    assert_eq!(
        broad.ownership_of("/p/exact/readme.md"),
        ScopeOwnership::Unique(SourceAssignment {
            scope: "broad".to_string(),
            kind: SourceKind::Embedded("custom".to_string()),
        })
    );
    assert_eq!(
        broad.ownership_of("/p/outside/component.vue"),
        ScopeOwnership::Unmatched
    );

    let conflicting_resolvers = ScopeDocuments(vec![(
        "frontend".to_string(),
        vec![
            ScopeDocument {
                kind: SourceKind::Dsql,
                paths: vec!["/p/mixed".to_string()],
            },
            ScopeDocument {
                kind: SourceKind::Embedded("custom".to_string()),
                paths: vec!["/p/mixed".to_string()],
            },
        ],
    )]);
    assert_eq!(
        conflicting_resolvers.ownership_of("/p/mixed/source.any"),
        ScopeOwnership::Ambiguous(vec![
            SourceAssignment {
                scope: "frontend".to_string(),
                kind: SourceKind::Dsql,
            },
            SourceAssignment {
                scope: "frontend".to_string(),
                kind: SourceKind::Embedded("custom".to_string()),
            },
        ])
    );
}
