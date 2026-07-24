use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bowl::Bowl;
use dsql_core::catalog::{
    Catalog, DatabaseMetadata, SchemaMetadata, TableMetadata, table_metadata_from_yaml,
};
use dsql_core::embedding::ExtractionRegistry;
use dsql_core::input::{LanguageDocument, LanguageInputs, populate_language_bowl};
use dsql_core::language_bowl;
use dsql_core::source::{ResolutionScope, ScopeDocuments, ScopeImports, SourceKind};
use dsql_generate::publish::MatchLockMode;
use dsql_generate::{GenerateOptions, ProjectContract, assemble_bowl, generate_project};
use dsql_project::Project;

const SHARED_SOURCE: &str =
    include_str!("../../../dsql-project/tests/it/fixture/scoped/queries/shared/fragments.dsql");
const FRONTEND_SOURCE: &str =
    include_str!("../../../dsql-project/tests/it/fixture/scoped/queries/frontend/titles.dsql");
const HOST_SOURCE: &str =
    include_str!("../../../dsql-project/tests/it/fixture/scoped/src/components/TitlePanel.ts");
const KIND_TYPE_SCHEMA: &str =
    include_str!("../../../dsql-project/tests/it/fixture/scoped/dsql/schema/public/kind_type.yaml");
const TITLE_SCHEMA: &str =
    include_str!("../../../dsql-project/tests/it/fixture/scoped/dsql/schema/public/title.yaml");
const NUMERIC_SCHEMA: &str = r#"---
schema: public
name: metrics
object_type: table
columns:
  - name: amount
    database_type: numeric
    data_type: numeric
    not_null: true
  - name: ratio
    database_type: float8
    data_type: float
    not_null: false
constraints: []
foreign_keys: []
indexes: []
"#;
const USERS_SCHEMA: &str = r#"---
schema: public
name: users
object_type: table
columns:
  - name: id
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: name
    database_type: text
    data_type: text
    not_null: true
constraints:
  - name: users_pkey
    kind: primary_key
    columns: [id]
foreign_keys: []
indexes:
  - name: users_pkey
    columns: [id]
    unique: true
"#;
const POSTS_SCHEMA: &str = r#"---
schema: public
name: posts
object_type: table
columns:
  - name: id
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: user_id
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: title
    database_type: text
    data_type: text
    not_null: false
  - name: created_at
    database_type: timestamptz
    data_type: timestamptz
    not_null: true
constraints:
  - name: posts_pkey
    kind: primary_key
    columns: [id]
foreign_keys:
  - name: posts_user_id_fkey
    columns: [user_id]
    references:
      schema: public
      table: users
      columns: [id]
indexes:
  - name: posts_pkey
    columns: [id]
    unique: true
"#;
const MEMBERSHIPS_SCHEMA: &str = r#"---
schema: public
name: memberships
object_type: table
columns:
  - name: tenant_id
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: user_id
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: locale
    database_type: text
    data_type: text
    not_null: true
constraints:
  - name: memberships_pkey
    kind: primary_key
    columns: [tenant_id, user_id]
foreign_keys: []
indexes:
  - name: memberships_pkey
    columns: [tenant_id, user_id]
    unique: true
"#;

#[test]
fn project_contract_is_canonical_and_generates_typed_source() {
    let first = ProjectContract::from_imports(&ScopeImports(BTreeMap::from([
        (
            "frontend".to_string(),
            vec!["shared".to_string(), "common".to_string()],
        ),
        ("shared".to_string(), Vec::new()),
        ("common".to_string(), Vec::new()),
    ])))
    .expect("first project contract");
    let second = ProjectContract::from_imports(&ScopeImports(BTreeMap::from([
        ("common".to_string(), Vec::new()),
        ("shared".to_string(), Vec::new()),
        (
            "frontend".to_string(),
            vec!["common".to_string(), "shared".to_string()],
        ),
    ])))
    .expect("equivalent project contract");

    // Import order can affect traversal order, but not the generated target
    // topology or its typed renderer contract.
    assert_eq!(first.fingerprint, second.fingerprint);
    insta::assert_snapshot!(
        first
            .typescript_source()
            .expect("TypeScript project contract renders")
    );
}

fn document(path: &str, text: &str, scope: &str) -> LanguageDocument {
    LanguageDocument {
        path: path.to_string(),
        text: text.to_string(),
        scope: ResolutionScope(scope.to_string()),
        kind: SourceKind::Dsql,
    }
}

fn scoped_documents() -> Vec<LanguageDocument> {
    vec![
        document("queries/shared/fragments.dsql", SHARED_SOURCE, "shared"),
        document("queries/frontend/titles.dsql", FRONTEND_SOURCE, "frontend"),
        LanguageDocument {
            path: "src/components/TitlePanel.ts".to_string(),
            text: HOST_SOURCE.to_string(),
            scope: ResolutionScope("frontend".to_string()),
            kind: SourceKind::Embedded("typescript".to_string()),
        },
    ]
}

fn catalog_from_tables(tables: impl IntoIterator<Item = &'static str>) -> Catalog {
    let mut tables = tables
        .into_iter()
        .map(|raw| table_metadata_from_yaml(raw).expect("embedded table metadata parses"))
        .collect::<Vec<TableMetadata>>();
    tables.sort_by(|left, right| left.name.cmp(&right.name));
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables,
        }],
        types: Vec::new(),
    }
    .into_catalog()
    .expect("embedded catalog builds")
}

fn scoped_catalog() -> Catalog {
    catalog_from_tables([KIND_TYPE_SCHEMA, TITLE_SCHEMA])
}

async fn memory_bowl(
    catalog: Catalog,
    documents: Vec<LanguageDocument>,
    imports: BTreeMap<String, Vec<String>>,
) -> Bowl {
    let bowl = language_bowl().await;
    populate_language_bowl(
        &bowl,
        LanguageInputs {
            catalog,
            documents,
            scope_imports: ScopeImports(imports),
            scope_documents: ScopeDocuments::default(),
            extraction_registry: ExtractionRegistry::default(),
            lint: None,
        },
    )
    .await;
    bowl
}

/// Copies the dsql-project scoped fixture into a temp dir so generation
/// can write its build/ tree without polluting the repository.
async fn fixture_project(test: &str) -> (PathBuf, Project) {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
    let dir = std::env::temp_dir().join(format!("dsql-generate-{test}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fixture copy");
    }
    copy_tree(&source, &dir);
    let project = Project::load_from(&dir)
        .await
        .expect("fixture project loads");
    (dir, project)
}

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("fixture dir");
    for entry in std::fs::read_dir(source).expect("fixture readable") {
        let entry = entry.expect("fixture entry");
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("fixture file copies");
        }
    }
}

#[tokio::test]
async fn generates_manifest_and_artifacts() {
    /// Hashes and hashed paths churn with any metadata change; the layout
    /// is what this snapshot pins.
    fn redact_hashes(manifest: &str) -> String {
        let mut redacted = String::new();
        for (index, piece) in manifest.split('"').enumerate() {
            if index > 0 {
                redacted.push('"');
            }
            if piece.len() == 64 && piece.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                redacted.push_str("<hash>");
            } else if piece.contains('.')
                && piece.split('.').nth(1).is_some_and(|address| {
                    address.len() == 16 && address.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                let mut parts = piece.split('.');
                let stem = parts.next().unwrap_or_default();
                let extension = parts.nth(1).unwrap_or_default();
                redacted.push_str(&format!("{stem}.<address>.{extension}"));
            } else {
                redacted.push_str(piece);
            }
        }
        redacted
    }

    let (dir, project) = fixture_project("artifacts").await;
    // A fragment composed of other fragments, spread at the root (empty
    // path) and inside a relation (nested path): renderers reuse types
    // from exactly this provenance.
    std::fs::write(
        dir.join("queries/shared/panels.dsql"),
        "fragment KindNameBits on kind_type {\n  kind\n}\n\n\
         fragment PanelBits on title {\n  ...TitleBits\n  panel_kind: kind_type {\n    ...KindNameBits\n  }\n}\n",
    )
    .expect("write composed fragments");
    let output = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
        MatchLockMode::Update,
    )
    .await
    .expect("generation succeeds");

    assert!(output.manifest_path.exists(), "immutable manifest exists");
    assert!(
        output.current_manifest_path.exists(),
        "the pointer manifest exists"
    );
    assert_eq!(output.generation_id, 1, "a fresh tree starts at 1");
    let manifest = std::fs::read_to_string(&output.manifest_path).expect("manifest readable");
    assert_eq!(
        manifest,
        std::fs::read_to_string(&output.current_manifest_path).expect("pointer readable"),
        "the pointer carries the same document"
    );
    insta::assert_snapshot!("manifest", redact_hashes(&manifest));

    // Consumers follow manifest entry paths — content-addressed files are
    // never discovered by globbing.
    let parsed: dsql_metadata::BuildManifest =
        facet_json::from_str(&manifest).expect("manifest parses");
    let entry_path = |name: &str| {
        parsed
            .operations
            .iter()
            .map(|entry| (entry.name.clone(), entry.path.clone()))
            .chain(
                parsed
                    .fragments
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.path.clone())),
            )
            .find(|(entry_name, _)| entry_name == name)
            .map(|(_, path)| project.root.join("build").join(path))
            .expect("manifest names the artifact")
    };
    let operation =
        std::fs::read_to_string(entry_path("Titles")).expect("operation artifact written");
    insta::assert_snapshot!("operation", operation);

    let fragment =
        std::fs::read_to_string(entry_path("TitleBits")).expect("fragment artifact written");
    insta::assert_snapshot!("fragment", fragment);

    // The composed fragment records its spread provenance: the root
    // spread at the empty path, the nested one at its relation path.
    let composed =
        std::fs::read_to_string(entry_path("PanelBits")).expect("composed fragment written");
    insta::assert_snapshot!("composed_fragment", composed);

    // The embedded operation source-maps into its host .ts file and
    // records the fragments its result paths came from.
    let embedded =
        std::fs::read_to_string(entry_path("TitlePanel")).expect("embedded artifact written");
    insta::assert_snapshot!("embedded_operation", embedded);

    // Second run: everything unchanged, nothing rewritten.
    let rerun = generate_project(
        &project,
        GenerateOptions {
            collection_limit: Some(10),
        },
        MatchLockMode::Update,
    )
    .await
    .expect("rerun succeeds");
    assert!(
        rerun
            .written
            .iter()
            .all(|path| !path.to_string_lossy().contains("operations/")
                && !path.to_string_lossy().contains("fragments/")),
        "unchanged artifact files must be skipped (manifests still commit)"
    );
    assert_eq!(
        rerun.generation_id,
        output.generation_id + 1,
        "every explicit generate commits a fresh generation"
    );

    std::fs::remove_dir_all(&dir).expect("fixture cleanup");
}

#[tokio::test]
async fn error_diagnostics_fail_generation() {
    let bowl = memory_bowl(
        scoped_catalog(),
        vec![document(
            "queries/frontend/broken.dsql",
            "query Broken {\n  missing_table {\n    id\n  }\n}\n",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("diagnostics must fail generation");
    assert!(
        error.to_string().contains("missing_table"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn duplicate_anonymous_variables_fail_before_publication() {
    let bowl = memory_bowl(
        scoped_catalog(),
        vec![document(
            "queries/frontend/anonymous.dsql",
            "query Ambiguous {\n  title(where .id > $ and .id < $ limit 1) {\n    id\n  }\n}\n",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("duplicate anonymous variables must fail generation");
    let message = error.to_string();
    assert!(
        message.contains("multiple anonymous variables")
            && message.contains("input.title.clause.where.id"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn generated_scope_groups_include_transitive_import_artifacts() {
    let bowl = memory_bowl(
        scoped_catalog(),
        scoped_documents(),
        BTreeMap::from([
            ("frontend".to_string(), vec!["middle".to_string()]),
            ("middle".to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), Vec::new()),
            ("standalone".to_string(), Vec::new()),
        ]),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("assembly succeeds");
    let frontend = assembled
        .snapshot
        .groups
        .iter()
        .find(|group| group.name == "frontend")
        .expect("frontend group");
    assert_eq!(frontend.imports, vec!["middle"]);
    assert!(frontend.generation_target);
    assert!(
        frontend
            .artifacts
            .iter()
            .any(|artifact| artifact == "shared/fragment/TitleBits"),
        "frontend closure includes shared artifacts: {:?}",
        frontend.artifacts
    );
    let shared = assembled
        .snapshot
        .groups
        .iter()
        .find(|group| group.name == "shared")
        .expect("shared group");
    assert!(!shared.generation_target);
    let standalone = assembled
        .snapshot
        .groups
        .iter()
        .find(|group| group.name == "standalone")
        .expect("empty terminal group");
    assert!(standalone.generation_target);
    assert!(standalone.artifacts.is_empty());
    assert_eq!(
        assembled
            .snapshot
            .project_contract
            .scopes
            .iter()
            .filter(|scope| scope.generation_target)
            .map(|scope| scope.name.as_str())
            .collect::<Vec<_>>(),
        ["frontend", "standalone"]
    );
}

#[tokio::test]
async fn configured_scope_graph_rejects_artifacts_from_unknown_scopes() {
    let bowl = memory_bowl(
        scoped_catalog(),
        vec![document(
            "queries/frontend/titles.dsql",
            "query Scoped { title(limit 1) { id } }\n",
            "frontend",
        )],
        BTreeMap::from([("configured".to_string(), Vec::new())]),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("configured graphs must not widen around artifacts");

    assert_eq!(
        error.to_string(),
        "artifact `frontend/operation/Scoped` belongs to scope `frontend`, which is absent from the configured graph"
    );
}

#[tokio::test]
async fn filter_match_lock_records_effective_scopes_and_resolved_identities() {
    let documents = vec![
        document(
            "queries/shared/policies.dsql",
            indoc::indoc! {r#"
                condition Allowed { where $:allowed }
                filter SharedStructural on { .id: uuid } {
                  apply where Allowed
                  field id where Allowed
                }
            "#},
            "shared",
        ),
        document(
            "queries/frontend/policies.dsql",
            "filter Same on users { where .id == .id }\n",
            "frontend",
        ),
        document(
            "queries/backend/policies.dsql",
            "filter Same on posts { where .id == .id }\n",
            "backend",
        ),
    ];
    let imports = BTreeMap::from([
        ("frontend".to_string(), vec!["shared".to_string()]),
        ("backend".to_string(), vec!["shared".to_string()]),
        ("shared".to_string(), Vec::new()),
    ]);
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        documents.clone(),
        imports.clone(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("filter lock assembles");
    let yaml = assembled
        .snapshot
        .filter_match_lock
        .to_yaml()
        .expect("filter lock serializes");

    let mut reversed = documents;
    reversed.reverse();
    let reversed = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        reversed,
        imports,
    )
    .await;
    let reversed = assemble_bowl(&reversed, None, GenerateOptions::default())
        .await
        .expect("reordered inputs assemble")
        .snapshot
        .filter_match_lock
        .to_yaml()
        .expect("reordered lock serializes");
    assert_eq!(yaml, reversed, "input ordering cannot change the lock");

    insta::assert_snapshot!(yaml);
}

#[tokio::test]
async fn numeric_wire_types_flow_through_generated_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([NUMERIC_SCHEMA]),
        vec![document(
            "queries/frontend/numeric.dsql",
            indoc::indoc! {r#"
                query NumericMetrics($$minimum = 12345678901234567890.12345678901234567890) {
                  metrics(where .amount >= $$minimum and .amount in $$amounts) { amount ratio }
                }
                query NumericSummary {
                  summary: metrics | aggregate {
                    total_amount: sum .amount
                    average_amount: avg .amount
                    total_ratio: sum .ratio
                    average_ratio: avg .ratio
                  }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("assembly succeeds");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| matches!(artifact.name.as_str(), "NumericMetrics" | "NumericSummary"))
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

#[tokio::test]
async fn defaults_and_fragment_lifting_flow_through_operation_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/variable-contracts.dsql",
            indoc::indoc! {r#"
                fragment PostWindow($$after? = null $$limit = 5) on users {
                  posts(where .created_at >= $$after limit $$) { id }
                }
                query Contained { users { ...PostWindow } }
                query Bound($$since = "2020-01-01T00:00:00Z") {
                  users { ...PostWindow($$after <- $$since) }
                }
                query NullableCollection($$ids? = null) {
                  users(where .id in $$ids) { id }
                }
                query CollectionDefault(
                  $$ids = ["00000000-0000-0000-0000-000000000001"]
                ) {
                  users(where .id in $$ids) { id }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("variable contracts assemble");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| {
            matches!(
                artifact.name.as_str(),
                "PostWindow" | "Contained" | "Bound" | "NullableCollection" | "CollectionDefault"
            )
        })
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

#[tokio::test]
async fn trusted_context_flows_through_sql_and_operation_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/context.dsql",
            indoc::indoc! {r#"
                filter CurrentUserOnly on users {
                  apply where true
                  where .id == $:user_id
                }
                query CurrentUser { users(limit 1) { id name } }
                fragment CurrentUserPosts on users {
                  posts(where .user_id == $:user_id) { id }
                }
                fragment CurrentUserPanel on users { ...CurrentUserPosts }
                query FragmentContext { users(limit 1) { ...CurrentUserPanel } }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("server-bound trusted context is valid query input");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| matches!(artifact.name.as_str(), "CurrentUser" | "FragmentContext"))
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

#[tokio::test]
async fn policy_metadata_explains_composed_access_and_disabled_assignments() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/access.dsql",
            indoc::indoc! {r#"
                filter ContextName on users {
                  apply
                  field name where $:is_admin
                }
                filter RowName on users {
                  apply
                  field name where .id == $:user_id
                }
                filter UserPosts on users {
                  apply
                  field posts where .id == $:user_id
                }
                filter VisiblePosts on posts {
                  apply
                  where $:can_read_posts
                }
                filter Manual on users { where .id == .id }
                query PolicyAudit(filter Manual when false) {
                  users(limit 1) {
                    name
                    posts { title }
                  }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("policy metadata assembles");
    let artifact = assembled
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "PolicyAudit")
        .expect("policy audit operation");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn singular_selection_shapes_flow_through_sql_and_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA, MEMBERSHIPS_SCHEMA]),
        vec![document(
            "queries/frontend/singular.dsql",
            indoc::indoc! {r#"
                query ByLimit { users(limit 1) { id name } }
                query ByKey { users(where .id == $$id) { id name } }
                query ByCompositeKey {
                  memberships(where .tenant_id == $$tenant and .user_id == $$user) { locale }
                }
                query RuntimeLimit { users(limit $$count) { id } }
                query NestedLimit {
                  users(limit 1) {
                    id
                    latest_post: posts(order by created_at desc limit 1) { title }
                  }
                }
                query FlattenedSingular { ...users(limit 1) { user_id: id name } }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("assembly succeeds");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == "query")
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

#[tokio::test]
async fn row_filtered_singular_relations_make_object_and_flattened_fields_nullable() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/filtered-owner.dsql",
            indoc::indoc! {r#"
                filter VisibleUsers on users { where .name != "hidden" }
                query FilteredOwner {
                  posts(limit 1) {
                    id
                    users(filter VisibleUsers) { id name }
                    ...users(filter VisibleUsers) { owner_id: id owner_name: name }
                  }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("row-filtered singular relation assembles");
    let artifact = assembled
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "FilteredOwner")
        .expect("filtered-owner operation");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn field_filter_types_are_conservative_across_fragment_consumers() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![
            document(
                "queries/shared/fragments.dsql",
                indoc::indoc! {r#"
                    fragment UserFields on users {
                      id
                      name
                      posts { id title }
                      post_stats: posts | aggregate { count latest: max .created_at }
                    }
                    fragment PostFields on posts {
                      id
                      users { id name }
                      ...users { owner_name: name }
                    }
                "#},
                "shared",
            ),
            document(
                "queries/frontend/filtered.dsql",
                indoc::indoc! {r#"
                    filter UserPrivacy on users {
                      apply where true
                      field name, posts where $:can_read_users
                    }
                    filter PostPrivacy on posts {
                      apply where true
                      field users where $:can_read_users
                    }
                    query FilteredUsers { users { ...UserFields } }
                    query FilteredPosts { posts { ...PostFields } }
                "#},
                "frontend",
            ),
            document(
                "queries/backend/unfiltered.dsql",
                indoc::indoc! {r#"
                    query UnfilteredUsers { users { ...UserFields } }
                    query UnfilteredPosts { posts { ...PostFields } }
                "#},
                "backend",
            ),
        ],
        BTreeMap::from([
            ("frontend".to_string(), vec!["shared".to_string()]),
            ("backend".to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), Vec::new()),
        ]),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("field-filtered fragment consumers assemble");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| {
            matches!(
                artifact.name.as_str(),
                "UserFields"
                    | "PostFields"
                    | "FilteredUsers"
                    | "FilteredPosts"
                    | "UnfilteredUsers"
                    | "UnfilteredPosts"
            )
        })
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

#[tokio::test]
async fn field_filter_context_conflicts_only_fail_when_both_guards_are_reached() {
    const FILTERS: &str = indoc::indoc! {r#"
        filter NameAccess on users {
          apply
          field name where .name == $:shared
        }
        filter IdAccess on users {
          apply
          field id where .id == $:shared
        }
    "#};

    let unused = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/unused-context.dsql",
            &format!("{FILTERS}query Unused {{ users(limit 1) {{ posts {{ title }} }} }}\n"),
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let unused = assemble_bowl(&unused, None, GenerateOptions::default())
        .await
        .expect("unused field guards do not conflict");
    let unused = unused
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "Unused")
        .expect("unused operation artifact");
    let unused: dsql_metadata::OperationMetadata =
        facet_json::from_str(&unused.serialized).expect("unused metadata parses");
    assert!(unused.context.is_empty());

    let one = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/one-context.dsql",
            &format!("{FILTERS}query One {{ users(limit 1) {{ name }} }}\n"),
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let one = assemble_bowl(&one, None, GenerateOptions::default())
        .await
        .expect("one reached field guard has one context type");
    let one = one
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "One")
        .expect("single-guard operation artifact");
    let one: dsql_metadata::OperationMetadata =
        facet_json::from_str(&one.serialized).expect("single-guard metadata parses");
    assert_eq!(one.context.len(), 1);
    assert_eq!(one.context[0].path, "context.shared");
    assert_eq!(one.context[0].data_type, "text");

    let both = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/conflicting-context.dsql",
            &format!("{FILTERS}query Both {{ users(limit 1) {{ id name }} }}\n"),
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&both, None, GenerateOptions::default())
        .await
        .expect_err("two reached incompatible guards fail generation");
    let message = error.to_string();
    assert!(
        message.contains("context.shared")
            && message.contains("incompatible")
            && message.contains("uuid")
            && message.contains("text"),
        "unexpected context conflict: {message}",
    );
}

#[tokio::test]
async fn aggregate_objects_flow_through_operation_and_fragment_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/aggregates.dsql",
            indoc::indoc! {r#"
                fragment UserStats on users {
                  post_stats: posts(where .title == $$title) | aggregate {
                    count
                    latest: max .created_at
                  }
                  post_groups: posts | aggregate by title_group: .title {
                    count
                    latest_group: max .created_at
                  }
                }
                fragment FlatPostStats on users {
                  ...posts(where .title == $$flat_title) | aggregate {
                    flat_post_count: count
                    flat_latest: max .created_at
                  }
                }
                query RootStats {
                  user_stats: users(where .name == $$name) | aggregate {
                    count
                    first_name: min .name
                  }
                }
                query GroupedRoot {
                  user_groups: users | aggregate by label: .name {
                    count
                    latest_name: max .name
                  }
                }
                query NestedStats {
                  users(limit 1) {
                    id
                    ...UserStats
                  }
                }
                query FlattenRoot {
                  ...users(where .name == $$flat_name) | aggregate {
                    user_count: count
                    first_name: min .name
                  }
                }
                query FlattenNested {
                  accounts: users(limit 1) {
                    id
                    ...FlatPostStats
                  }
                }
                query FlattenOwner {
                  feed: posts(limit 1) {
                    id
                    ...users(where .name == $$owner_name) {
                      owner_name: name
                      owner_posts: posts(limit 1) { title }
                      ...posts | aggregate { owner_post_count: count }
                    }
                  }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(
        &bowl,
        None,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect("assembly succeeds");
    let mut artifacts = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| {
            matches!(
                artifact.name.as_str(),
                "FlatPostStats"
                    | "GroupedRoot"
                    | "FlattenNested"
                    | "FlattenOwner"
                    | "FlattenRoot"
                    | "NestedStats"
                    | "RootStats"
                    | "UserStats"
            )
        })
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

/// Two *independent* scopes may each define an operation with the same
/// public name — resolution namespaces are separate, and without an
/// import relationship the language checks rightly stay silent (importing
/// scopes now get a check-time collision diagnostic instead) — but the
/// build tree currently uses a flat per-kind namespace: generation must
/// refuse before writing rather than let the later artifact overwrite
/// the earlier one.
#[tokio::test]
async fn colliding_operation_names_refuse_before_writing() {
    let query = "query Collide {\n  title(limit 1) {\n    id\n  }\n}\n";
    let bowl = memory_bowl(
        scoped_catalog(),
        vec![
            document("queries/shared/collide.dsql", query, "shared"),
            document("queries/api/collide.dsql", query, "api"),
        ],
        BTreeMap::from([
            ("api".to_string(), Vec::new()),
            ("shared".to_string(), Vec::new()),
        ]),
    )
    .await;
    let error = assemble_bowl(
        &bowl,
        None,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect_err("colliding names must refuse generation");

    let message = error.to_string();
    assert!(
        message.contains("Collide") && message.contains("both write"),
        "collision error names the artifacts, got: {message}"
    );
    assert!(
        message.contains("shared/collide.dsql") && message.contains("api/collide.dsql"),
        "collision error names both sources, got: {message}"
    );
}

/// Names differing only by case are distinct to the language but alias
/// one file on case-insensitive filesystems, so they collide too.
#[tokio::test]
async fn case_folded_operation_names_refuse_before_writing() {
    let bowl = memory_bowl(
        scoped_catalog(),
        vec![
            document(
                "queries/shared/upper.dsql",
                "query Collide {
  title(limit 1) {
    id
  }
}
",
                "shared",
            ),
            document(
                "queries/api/lower.dsql",
                "query collide {
  title(limit 1) {
    id
  }
}
",
                "api",
            ),
        ],
        BTreeMap::from([
            ("api".to_string(), Vec::new()),
            ("shared".to_string(), Vec::new()),
        ]),
    )
    .await;
    let error = assemble_bowl(
        &bowl,
        None,
        GenerateOptions {
            collection_limit: Some(10),
        },
    )
    .await
    .expect_err("case-folded names must refuse generation");

    let message = error.to_string();
    assert!(
        message.contains("Collide") && message.contains("collide"),
        "collision error names both artifacts, got: {message}"
    );
}
