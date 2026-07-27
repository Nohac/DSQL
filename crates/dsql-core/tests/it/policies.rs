//! Filter declarations, structural target resolution, reusable conditions,
//! assignments, and their temporary fail-closed trusted-context boundary.

use std::collections::BTreeMap;

use bowl::{Query, Singleton};
use dsql_core::catalog::{
    ColumnMetadata, DataType, DatabaseMetadata, ObjectType, SchemaMetadata, TableMetadata, TypeKey,
    insert_catalog,
};
use dsql_core::entities::policy::PolicyIndex;
use dsql_core::facts::{PlanDemand, arm_editor_demands};
use dsql_core::language_bowl;
use dsql_core::source::{
    ResolutionScope, ScopeImports, SourceKind, insert_embedding_source, insert_source,
    insert_source_scoped,
};

use crate::{imdb_catalog, render_diagnostic_facts, replace_source_text};

async fn render_index(bowl: &bowl::Bowl, catalog: &dsql_core::catalog::Catalog) -> String {
    let rows = bowl.scoop::<Query<(bowl::Entity, &PolicyIndex)>>().await;
    let mut rendered = rows
        .collect()
        .into_iter()
        .flat_map(|(_, index)| &index.entries)
        .map(|entry| {
            let mut tables = entry
                .matches
                .iter()
                .filter_map(|table| catalog.table_by_id(*table))
                .map(|table| format!("{}::{}", table.schema, table.name))
                .collect::<Vec<_>>();
            tables.sort();
            format!(
                "{} {} [{}] default={} enforced={}",
                entry.kind,
                entry.name,
                tables.join(", "),
                entry.default_active,
                entry.always_enforced,
            )
        })
        .collect::<Vec<_>>();
    rendered.sort();
    rendered.join("\n")
}

#[tokio::test]
async fn filters_and_conditions_resolve_concrete_and_structural_targets() {
    let bowl = language_bowl().await;
    let catalog = imdb_catalog();
    insert_catalog(&bowl, catalog.clone()).await;
    insert_source(
        &bowl,
        "policies.dsql",
        indoc::indoc! {r#"
            condition Enabled { where true }
            condition SameRow on { .id: int } { where .id == .id }
            filter Titles on public::title {
              apply where Enabled
              where .id > 0
              field production_year, kind_id where SameRow
            }
            filter IntegerRows on { .id: int } { where .id > 0 }
            filter Correlated on public::title {
              where exists .movie_info_idx(where .info_type_id == ..kind_id)
                and exists public::info_type(where .id == ..kind_id)
              field movie_info_idx where true
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_index(&bowl, &catalog).await);
}

#[tokio::test]
async fn invalid_filter_definitions_and_assignments_fail_closed() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_source(
        &bowl,
        "invalid-policies.dsql",
        indoc::indoc! {r#"
            condition RowBound on { .id: int } { where .id > 0 }
            condition MissingTarget { where .id > 0 }
            filter MissingTarget { where true }
            filter EmptyShape on {} { where true }
            filter UnknownType on { .id: mystery } { where true }
            filter Undeclared on { .id: int } { where .kind_id > 0 }
            filter Enforced on public::title { apply where true where .id > $:minimum_id }
            filter WrongInput on public::title { where .id == $$public_id }
            filter InvalidApply on public::title { apply where RowBound where .id > 0 }
            filter InvalidExists on public::title { where exists .kind(where .missing == ..missing) }
            filter StructuralExists on { .id: int } { where exists public::title(where .id == ..id) }
            filter NestedAssignment on public::title { where exists .movie_info_idx(filter Enforced) }
            query Invalid(
              filter Missing
              filter Enforced when $$disable
            ) {
              title(filter Enforced when false filter Enforced) { id }
              name(filter Enforced) { id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn structural_policy_types_accept_builtin_aliases_only() {
    let bowl = language_bowl().await;
    let column = |name: &str, database_type: &str| ColumnMetadata {
        name: name.to_string(),
        description: None,
        provider_type: TypeKey::new("pg_catalog", database_type),
        database_type: database_type.to_string(),
        data_type: DataType::from_database_type(database_type),
        not_null: true,
    };
    let catalog = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![TableMetadata {
                schema: "public".to_string(),
                name: "records".to_string(),
                object_type: ObjectType::Table,
                description: None,
                columns: vec![
                    column("integer_value", "int8"),
                    column("float_value", "float8"),
                    column("text_value", "varchar"),
                    column("json_value", "jsonb"),
                    column("custom", "citext"),
                ],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("provider-only type catalog builds");
    insert_catalog(&bowl, catalog).await;
    arm_editor_demands(&bowl).await;
    insert_source(
        &bowl,
        "type-aliases.dsql",
        indoc::indoc! {r#"
            filter CanonicalInt on { .integer_value: int } { where true }
            filter CanonicalFloat on { .float_value: float } { where true }
            filter DatabaseInt on { .integer_value: int8 } { where true }
            filter DatabaseText on { .text_value: varchar } { where true }
            filter DatabaseJson on { .json_value: jsonb } { where true }
            filter ProviderOnly on { .custom: citext } { where true }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn filter_visibility_follows_resolution_scope_imports() {
    async fn bowl(imports: &[(&str, &[&str])]) -> bowl::Bowl {
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        arm_editor_demands(&bowl).await;
        let imports = imports
            .iter()
            .map(|(scope, imports)| {
                (
                    (*scope).to_string(),
                    imports.iter().map(|import| (*import).to_string()).collect(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        bowl.insert((Singleton::<ScopeImports>::new(), ScopeImports(imports)))
            .await;
        bowl
    }

    async fn insert(bowl: &bowl::Bowl, path: &str, scope: &str, source: &str) {
        insert_source_scoped(
            bowl,
            path,
            source,
            ResolutionScope(scope.to_string()),
            SourceKind::Dsql,
        )
        .await;
    }

    let imported = bowl(&[("frontend", &["shared"]), ("shared", &[])]).await;
    insert(
        &imported,
        "shared.dsql",
        "shared",
        "filter Visible on title { where .id > 0 }",
    )
    .await;
    insert(
        &imported,
        "query.dsql",
        "frontend",
        "query Q { title(filter Visible) { id } }",
    )
    .await;

    let isolated = bowl(&[("api", &[]), ("shared", &[])]).await;
    insert(
        &isolated,
        "shared.dsql",
        "shared",
        "filter Visible on title { where .id > 0 }",
    )
    .await;
    insert(
        &isolated,
        "query.dsql",
        "api",
        "query Q { title(filter Visible) { id } }",
    )
    .await;

    let ambiguous = bowl(&[
        ("frontend", &["left", "right"]),
        ("left", &[]),
        ("right", &[]),
    ])
    .await;
    insert(
        &ambiguous,
        "left.dsql",
        "left",
        "filter Visible on title { where .id > 0 }",
    )
    .await;
    insert(
        &ambiguous,
        "right.dsql",
        "right",
        "filter Visible on title { where .id > 0 }",
    )
    .await;
    insert(
        &ambiguous,
        "query.dsql",
        "frontend",
        "query Q { title(filter Visible) { id } }",
    )
    .await;

    insta::assert_snapshot!(format!(
        "imported:\n{}\n\nisolated:\n{}\n\nambiguous:\n{}",
        render_diagnostic_facts(&imported).await,
        render_diagnostic_facts(&isolated).await,
        render_diagnostic_facts(&ambiguous).await,
    ));
}

#[tokio::test]
async fn filter_assignment_conditions_infer_boolean_inputs() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_source(
        &bowl,
        "inputs.dsql",
        indoc::indoc! {r#"
            filter Manual on title { where .id > 0 }
            filter Info on movie_info_idx { where .id > 0 }
            query Q(filter Manual when $$operation) {
              title(
                filter Manual when $local
                where exists .movie_info_idx(filter Info when $$nested)
              ) { id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(crate::variables::render_bindings(&bowl).await);
}

#[tokio::test]
async fn trusted_context_type_conflicts_fail_across_policy_and_operation_boundaries() {
    async fn case(source: &str) -> String {
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        arm_editor_demands(&bowl).await;
        bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
            .await;
        insert_source(&bowl, "context-conflict.dsql", source).await;
        render_diagnostic_facts(&bowl).await
    }

    let policies = case(indoc::indoc! {r#"
        filter ById on title { apply where true where .id > $:shared }
        filter ByName on title { apply where true where .title > $:shared }
        query Conflict { title { id } }
    "#})
    .await;
    let query = case(indoc::indoc! {r#"
        filter ById on title { apply where true where .id > $:shared }
        query Conflict { title(where .title > $:shared) { id } }
    "#})
    .await;
    let roots = case(indoc::indoc! {r#"
        filter TitleById on title { apply where true where .id > $:shared }
        filter InfoByValue on movie_info_idx { apply where true where .info > $:shared }
        query Conflict { title { id } movie_info_idx { id } }
    "#})
    .await;
    let fragments = case(indoc::indoc! {r#"
        fragment ByIdContext on title {
          by_id: movie_info_idx(where .id > $:shared) { id }
        }
        fragment ByTextContext on title {
          by_text: movie_info_idx(where .info > $:shared) { id }
        }
        query Conflict { title { ...ByIdContext ...ByTextContext } }
    "#})
    .await;

    insta::assert_snapshot!(format!(
        "policy-policy:\n{policies}\n\npolicy-query:\n{query}\n\ncross-root:\n{roots}\n\nfragments:\n{fragments}"
    ));
}

#[tokio::test]
async fn structural_matches_rederive_across_source_revisits() {
    let bowl = language_bowl().await;
    let catalog = imdb_catalog();
    insert_catalog(&bowl, catalog.clone()).await;
    let file = insert_source(
        &bowl,
        "policy.dsql",
        "filter Matching on { .id: int } { where .id > 0 }",
    )
    .await;
    let initial = render_index(&bowl, &catalog).await;

    replace_source_text(&bowl, file, ".id: int", ".missing: int").await;
    let changed = render_index(&bowl, &catalog).await;

    replace_source_text(&bowl, file, ".missing: int", ".id: int").await;
    let restored = render_index(&bowl, &catalog).await;

    bowl.entity(file).despawn().await;
    let removed = render_index(&bowl, &catalog).await;

    insta::assert_snapshot!(format!(
        "initial:\n{initial}\n\nchanged:\n{changed}\n\nrestored:\n{restored}\n\nremoved:\n{removed}"
    ));
}

#[tokio::test]
async fn compiled_filters_execute_across_supported_sources() {
    async fn case(policy: &str, query: &str) -> String {
        let bowl = language_bowl().await;
        insert_catalog(&bowl, imdb_catalog()).await;
        arm_editor_demands(&bowl).await;
        insert_source(&bowl, "case.dsql", &format!("{policy}\n{query}")).await;
        render_diagnostic_facts(&bowl).await
    }

    let manual = case(
        "filter Manual on title { where .id > 0 }",
        "query Manual { title(filter Manual) { id } }",
    )
    .await;
    let default = case(
        "filter Default on title {\n  apply\n  where .id > 0\n}",
        "query Default { title { id } }",
    )
    .await;
    let opt_out = case(
        "filter Default on title {\n  apply\n  where .id > 0\n}",
        "query OptOut { title(filter Default when false) { id } }",
    )
    .await;
    let aggregate = case(
        "filter Manual on title { where .id > 0 }",
        "query Aggregate { title(filter Manual) | aggregate { count } }",
    )
    .await;
    let exists = case(
        "filter Manual on movie_info_idx { where .id > 0 }",
        "query Exists { title(where exists .movie_info_idx(filter Manual)) { id } }",
    )
    .await;
    let unrelated = case(
        "filter Unrelated on name {\n  apply\n  where .id > 0\n}",
        "query Clean { title { id } }",
    )
    .await;
    let field = case(
        "filter HiddenYear on title { field production_year where true }",
        "query Field { title(filter HiddenYear) { production_year } }",
    )
    .await;

    insta::assert_snapshot!(format!(
        "manual:\n{manual}\n\ndefault:\n{default}\n\nopt out:\n{opt_out}\n\naggregate:\n{aggregate}\n\nexists:\n{exists}\n\nunrelated:\n{unrelated}\n\nfield:\n{field}"
    ));
}

#[tokio::test]
async fn operation_assignments_match_sources_contributed_by_fragments() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_source(
        &bowl,
        "fragment-filter.dsql",
        indoc::indoc! {r#"
            filter InfoRows on movie_info_idx { where .id > 0 }
            filter LocalInfo on movie_info_idx { where .id > 0 }
            fragment Info on title { movie_info_idx(filter LocalInfo) { id } }
            query Q(filter InfoRows) { title(limit 1) { ...Info } }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn embedded_filter_definitions_are_rejected() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_embedding_source(
        &bowl,
        "host.ts",
        "const policy = dsql`filter Visible on title { where .id > 0 }`;",
        "typescript",
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}
