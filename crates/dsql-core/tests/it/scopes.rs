//! Resolution scopes: fragments resolve across files within a scope and
//! through imports; independent scopes stay independent; collisions and
//! ambiguities are diagnostics (docs/spec/resolution-scopes.md).

use std::collections::BTreeMap;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::insert_catalog;
use dsql_core::entities::fragment_spread::ResolvedSpread;
use dsql_core::facts::DiagnosticsDemand;
use dsql_core::language_bowl;
use dsql_core::source::{ResolutionScope, ScopeImports, insert_source_scoped};
use futures::executor::block_on;

use crate::{imdb_catalog, render_diagnostic_facts};

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
    insert_source_scoped(bowl, path, text, ResolutionScope(scope.to_string())).await;
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

#[test]
fn fragments_resolve_across_files_within_a_scope() {
    block_on(async {
        let bowl = scoped_bowl(&[]).await;
        insert(&bowl, "fragments.dsql", "default", FRAGMENT).await;
        insert(&bowl, "query.dsql", "default", SPREAD).await;

        assert_eq!(resolutions(&bowl).await, 1);
        assert_eq!(render_diagnostic_facts(&bowl).await, "");
    });
}

#[test]
fn imported_scopes_provide_fragments() {
    block_on(async {
        let bowl = scoped_bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
        insert(&bowl, "shared.dsql", "shared", FRAGMENT).await;
        insert(&bowl, "page.dsql", "frontend", SPREAD).await;

        assert_eq!(resolutions(&bowl).await, 1);
        assert_eq!(render_diagnostic_facts(&bowl).await, "");
    });
}

#[test]
fn scopes_without_imports_do_not_see_each_other() {
    block_on(async {
        let bowl = scoped_bowl(&[("api", &[]), ("frontend", &[])]).await;
        insert(&bowl, "api.dsql", "api", FRAGMENT).await;
        insert(&bowl, "page.dsql", "frontend", SPREAD).await;

        assert_eq!(resolutions(&bowl).await, 0);
        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn same_name_in_independent_scopes_is_clean() {
    block_on(async {
        let bowl = scoped_bowl(&[("api", &[]), ("frontend", &[])]).await;
        insert(&bowl, "api.dsql", "api", FRAGMENT).await;
        insert(&bowl, "api-query.dsql", "api", SPREAD).await;
        insert(&bowl, "page-fragments.dsql", "frontend", FRAGMENT).await;
        insert(&bowl, "page.dsql", "frontend", SPREAD).await;

        assert_eq!(resolutions(&bowl).await, 2, "each scope resolves locally");
        assert_eq!(render_diagnostic_facts(&bowl).await, "");
    });
}

#[test]
fn local_fragment_colliding_with_import_is_reported() {
    block_on(async {
        let bowl = scoped_bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
        insert(&bowl, "shared.dsql", "shared", FRAGMENT).await;
        insert(&bowl, "page-fragments.dsql", "frontend", FRAGMENT).await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

#[test]
fn fragment_provided_by_two_imports_is_ambiguous_at_the_spread() {
    block_on(async {
        let bowl = scoped_bowl(&[("frontend", &["a", "b"]), ("a", &[]), ("b", &[])]).await;
        insert(&bowl, "a.dsql", "a", FRAGMENT).await;
        insert(&bowl, "b.dsql", "b", FRAGMENT).await;
        insert(&bowl, "page.dsql", "frontend", SPREAD).await;

        assert_eq!(resolutions(&bowl).await, 0);
        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

const QUERY: &str = "query Titles {\n  title(limit 1) {\n    id\n  }\n}\n";

/// A local query colliding with an imported one is a language error at
/// the local definition — mirrors the fragment rule.
#[test]
fn local_query_colliding_with_import_is_reported() {
    block_on(async {
        let bowl = scoped_bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
        insert(&bowl, "shared.dsql", "shared", QUERY).await;
        insert(&bowl, "page.dsql", "frontend", QUERY).await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// Two imported scopes providing one query name to a consuming scope
/// collide in its artifact closure even with no local definition or use
/// site — reported once, on the first provider, naming every provider.
/// (Fragments keep their spread-site ambiguity diagnostic instead.)
#[test]
fn query_provided_by_two_imports_collides_at_the_definition() {
    block_on(async {
        let bowl = scoped_bowl(&[("frontend", &["a", "b"]), ("a", &[]), ("b", &[])]).await;
        insert(&bowl, "a.dsql", "a", QUERY).await;
        insert(&bowl, "b.dsql", "b", QUERY).await;

        insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    });
}

/// The import-ambiguity check follows edits: introducing the second
/// provider after the first settle reports, renaming it away retires.
#[test]
fn import_ambiguities_follow_edits() {
    use bowl::{Mut, Query};
    use dsql_core::source::SourceText;

    block_on(async {
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
        let set_b = |text: String| {
            let bowl = &bowl;
            async move {
                let rows = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
                for (entity, slot) in rows.collect() {
                    if entity == target {
                        let text = text.clone();
                        slot.with_latest(move |slot| slot.set_text(&text)).await;
                    }
                }
            }
        };

        set_b(QUERY.to_string()).await;
        let reported = render_diagnostic_facts(&bowl).await;
        assert!(
            reported.contains("provided to scope `frontend`"),
            "the ambiguity appears after the edit, got: {reported:?}"
        );

        set_b("query Other {\n  title(limit 1) {\n    id\n  }\n}\n".to_string()).await;
        assert_eq!(
            render_diagnostic_facts(&bowl).await,
            "",
            "renaming away retires the ambiguity"
        );
    });
}

/// Scope ownership preserves its outcomes: unmatched and overlapping
/// paths must not silently collapse into the default scope.
#[test]
fn scope_ownership_distinguishes_outcomes() {
    use dsql_core::source::{ScopeDocuments, ScopeOwnership};

    let empty = ScopeDocuments::default();
    assert_eq!(
        empty.ownership_of("/p/queries/a.dsql"),
        ScopeOwnership::ImplicitDefault
    );

    let configured = ScopeDocuments(vec![
        (
            "shared".to_string(),
            vec!["/p/queries/shared/**/*.dsql".to_string()],
        ),
        (
            "frontend".to_string(),
            vec![
                "/p/queries/frontend/**/*.dsql".to_string(),
                "/p/queries/shared/both.dsql".to_string(),
            ],
        ),
    ]);
    assert_eq!(
        configured.ownership_of("/p/queries/frontend/new.dsql"),
        ScopeOwnership::Unique("frontend".to_string())
    );
    assert_eq!(
        configured.ownership_of("/p/other/loose.dsql"),
        ScopeOwnership::Unmatched
    );
    assert_eq!(
        configured.ownership_of("/p/queries/shared/both.dsql"),
        ScopeOwnership::Ambiguous(vec!["frontend".to_string(), "shared".to_string()])
    );
}
