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
use dsql_generate::{GenerateOptions, assemble_bowl, generate_project};
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
    assert!(
        frontend
            .artifacts
            .iter()
            .any(|artifact| artifact == "shared/fragment/TitleBits"),
        "frontend closure includes shared artifacts: {:?}",
        frontend.artifacts
    );
}

#[tokio::test]
async fn numeric_wire_types_flow_through_generated_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([NUMERIC_SCHEMA]),
        vec![document(
            "queries/frontend/numeric.dsql",
            concat!(
                "query NumericMetrics {\n",
                "  metrics(where .amount >= $$minimum and .amount in $$amounts) { amount ratio }\n",
                "}\n",
                "query NumericSummary {\n",
                "  summary: metrics | aggregate {\n",
                "    total_amount: sum .amount\n",
                "    average_amount: avg .amount\n",
                "    total_ratio: sum .ratio\n",
                "    average_ratio: avg .ratio\n",
                "  }\n",
                "}\n",
            ),
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
async fn trusted_context_flows_through_sql_and_operation_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/context.dsql",
            "query CurrentUser { users(where .id == $:user_id) { id name } }\n",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("server-bound trusted context is valid query input");
    let artifact = assembled
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "CurrentUser")
        .expect("current-user operation");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn singular_selection_shapes_flow_through_sql_and_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA, MEMBERSHIPS_SCHEMA]),
        vec![document(
            "queries/frontend/singular.dsql",
            concat!(
                "query ByLimit { users(limit 1) { id name } }\n",
                "query ByKey { users(where .id == $$id) { id name } }\n",
                "query ByCompositeKey {\n",
                "  memberships(where .tenant_id == $$tenant and .user_id == $$user) { locale }\n",
                "}\n",
                "query RuntimeLimit { users(limit $$count) { id } }\n",
                "query NestedLimit {\n",
                "  users(limit 1) {\n",
                "    id\n",
                "    latest_post: posts(order by created_at desc limit 1) { title }\n",
                "  }\n",
                "}\n",
                "query FlattenedSingular { ...users(limit 1) { user_id: id name } }\n",
            ),
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
async fn aggregate_objects_flow_through_operation_and_fragment_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/aggregates.dsql",
            concat!(
                "fragment UserStats on users {\n",
                "  post_stats: posts(where .title == $$title) | aggregate {\n",
                "    count\n",
                "    latest: max .created_at\n",
                "  }\n",
                "  post_groups: posts | aggregate by title_group: .title {\n",
                "    count\n",
                "    latest_group: max .created_at\n",
                "  }\n",
                "}\n",
                "fragment FlatPostStats on users {\n",
                "  ...posts(where .title == $$flat_title) | aggregate {\n",
                "    flat_post_count: count\n",
                "    flat_latest: max .created_at\n",
                "  }\n",
                "}\n",
                "query RootStats {\n",
                "  user_stats: users(where .name == $$name) | aggregate {\n",
                "    count\n",
                "    first_name: min .name\n",
                "  }\n",
                "}\n",
                "query GroupedRoot {\n",
                "  user_groups: users | aggregate by label: .name {\n",
                "    count\n",
                "    latest_name: max .name\n",
                "  }\n",
                "}\n",
                "query NestedStats {\n",
                "  users(limit 1) {\n",
                "    id\n",
                "    ...UserStats\n",
                "  }\n",
                "}\n",
                "query FlattenRoot {\n",
                "  ...users(where .name == $$flat_name) | aggregate {\n",
                "    user_count: count\n",
                "    first_name: min .name\n",
                "  }\n",
                "}\n",
                "query FlattenNested {\n",
                "  accounts: users(limit 1) {\n",
                "    id\n",
                "    ...FlatPostStats\n",
                "  }\n",
                "}\n",
                "query FlattenOwner {\n",
                "  feed: posts(limit 1) {\n",
                "    id\n",
                "    ...users(where .name == $$owner_name) {\n",
                "      owner_name: name\n",
                "      owner_posts: posts(limit 1) { title }\n",
                "      ...posts | aggregate { owner_post_count: count }\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
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
