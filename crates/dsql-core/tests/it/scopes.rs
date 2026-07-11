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
