use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bowl::{Bowl, Entity, Mut, Query};
use dsql_core::catalog::{
    Catalog, ColumnMetadata, DataType, DatabaseMetadata, EnumTypeMetadata, EnumVariantMetadata,
    ObjectType, ProviderTypeFacts, SchemaMetadata, TableMetadata, TypeKey, TypeMetadata,
    TypeStructureMetadata, table_metadata_from_yaml,
};
use dsql_core::embedding::ExtractionRegistry;
use dsql_core::input::{LanguageDocument, LanguageInputs, populate_language_bowl};
use dsql_core::language_bowl;
use dsql_core::source::{
    FilePath, ResolutionScope, ScopeDocuments, ScopeImports, SourceKind, SourceText,
};
use dsql_generate::publish::MatchLockMode;
use dsql_generate::{GenerateOptions, ProjectContract, assemble_bowl, generate_project};
use dsql_metadata::DefinitionKind;
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
    provider_type:
      schema: pg_catalog
      name: numeric
    database_type: numeric
    data_type: numeric
    not_null: true
  - name: ratio
    provider_type:
      schema: pg_catalog
      name: float8
    database_type: float8
    data_type: float
    not_null: false
constraints: []
foreign_keys: []
indexes: []
"#;
const PROVIDER_SCALAR_SCHEMA: &str = r#"---
schema: public
name: events
object_type: table
columns:
  - name: event_date
    provider_type:
      schema: pg_catalog
      name: date
    formatted_type: date
    database_type: date
    data_type: unknown
    not_null: true
  - name: event_dates
    provider_type:
      schema: pg_catalog
      name: _date
    formatted_type: "date[]"
    database_type: _date
    data_type: unknown
    not_null: true
  - name: address
    provider_type:
      schema: pg_catalog
      name: inet
    formatted_type: inet
    database_type: inet
    data_type: unknown
    not_null: true
  - name: big_id
    provider_type:
      schema: pg_catalog
      name: int8
    formatted_type: bigint
    database_type: int8
    data_type: bigint
    not_null: true
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
    provider_type:
      schema: pg_catalog
      name: uuid
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: name
    provider_type:
      schema: pg_catalog
      name: text
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
    access_method: btree
    keys:
      - column: id
    unique: true
  - name: users_name_search_idx
    access_method: gin
    keys:
      - column: name
        operator_class: public.gin_trgm_ops
        capabilities: [like]
    unique: false
"#;
const POSTS_SCHEMA: &str = r#"---
schema: public
name: posts
object_type: table
columns:
  - name: id
    provider_type:
      schema: pg_catalog
      name: uuid
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: user_id
    provider_type:
      schema: pg_catalog
      name: uuid
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: title
    provider_type:
      schema: pg_catalog
      name: text
    database_type: text
    data_type: text
    not_null: false
  - name: created_at
    provider_type:
      schema: pg_catalog
      name: timestamptz
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
    access_method: btree
    keys:
      - column: id
    unique: true
"#;

fn provider_scalar_metadata() -> DatabaseMetadata {
    let provider_type = |name: &str, category: &str| TypeMetadata {
        internal_type: name.to_string(),
        readable_type: name.to_string(),
        schema: "pg_catalog".to_string(),
        structure: TypeStructureMetadata::scalar(),
        provider: Some(ProviderTypeFacts {
            kind: "b".to_string(),
            category: category.to_string(),
            effective_kind: None,
            effective_category: None,
            orderable: true,
        }),
        operations: ["=", "<>", ">", ">=", "<", "<="]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
    };
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![
                table_metadata_from_yaml(PROVIDER_SCALAR_SCHEMA)
                    .expect("provider scalar table parses"),
            ],
        }],
        types: vec![
            provider_type("date", "D"),
            TypeMetadata {
                internal_type: "_date".to_string(),
                readable_type: "date[]".to_string(),
                schema: "pg_catalog".to_string(),
                structure: TypeStructureMetadata::array(TypeKey::new("pg_catalog", "date")),
                provider: Some(ProviderTypeFacts {
                    kind: "b".to_string(),
                    category: "A".to_string(),
                    effective_kind: None,
                    effective_category: None,
                    orderable: true,
                }),
                operations: ["=", "<>"].into_iter().map(str::to_string).collect(),
            },
            provider_type("inet", "I"),
            provider_type("int8", "N"),
        ],
    }
}

fn provider_scalar_catalog() -> Catalog {
    provider_scalar_metadata()
        .to_catalog()
        .expect("provider scalar catalog builds")
}

fn structured_type_catalog() -> Catalog {
    let provider_type =
        |schema: &str, name: &str, kind: &str, category: &str, structure: TypeStructureMetadata| {
            TypeMetadata {
                internal_type: name.to_string(),
                readable_type: if schema == "pg_catalog" {
                    name.to_string()
                } else {
                    format!("{schema}.{name}")
                },
                schema: schema.to_string(),
                structure,
                provider: Some(ProviderTypeFacts {
                    kind: kind.to_string(),
                    category: category.to_string(),
                    effective_kind: None,
                    effective_category: None,
                    orderable: true,
                }),
                operations: ["=", "<>", ">", ">=", "<", "<="]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            }
        };
    let column = |name: &str, provider_type: TypeKey, data_type: DataType| ColumnMetadata {
        name: name.to_string(),
        description: None,
        database_type: provider_type.name.clone(),
        provider_type,
        formatted_type: None,
        type_modifier: None,
        data_type,
        not_null: true,
    };
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![TableMetadata {
                schema: "public".to_string(),
                name: "typed_values".to_string(),
                object_type: ObjectType::Table,
                description: None,
                columns: vec![
                    column(
                        "label",
                        TypeKey::new("public", "label_domain"),
                        DataType::Text,
                    ),
                    column(
                        "address",
                        TypeKey::new("public", "address_domain"),
                        DataType::Unknown,
                    ),
                    column(
                        "labels",
                        TypeKey::new("pg_catalog", "_text"),
                        DataType::Unknown,
                    ),
                    column(
                        "big_values",
                        TypeKey::new("pg_catalog", "_int8"),
                        DataType::Unknown,
                    ),
                    column(
                        "nested_label",
                        TypeKey::new("public", "nested_label_domain"),
                        DataType::Text,
                    ),
                    column(
                        "domain_labels",
                        TypeKey::new("public", "labels_domain"),
                        DataType::Unknown,
                    ),
                    column(
                        "labeled_values",
                        TypeKey::new("public", "_label_domain"),
                        DataType::Unknown,
                    ),
                ],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: vec![
            provider_type(
                "pg_catalog",
                "text",
                "b",
                "S",
                TypeStructureMetadata::scalar(),
            ),
            provider_type(
                "pg_catalog",
                "int8",
                "b",
                "N",
                TypeStructureMetadata::scalar(),
            ),
            provider_type(
                "pg_catalog",
                "inet",
                "b",
                "I",
                TypeStructureMetadata::scalar(),
            ),
            provider_type(
                "public",
                "label_domain",
                "d",
                "S",
                TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "text")),
            ),
            provider_type(
                "public",
                "address_domain",
                "d",
                "I",
                TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "inet")),
            ),
            provider_type(
                "public",
                "nested_label_domain",
                "d",
                "S",
                TypeStructureMetadata::domain(TypeKey::new("public", "label_domain")),
            ),
            provider_type(
                "public",
                "labels_domain",
                "d",
                "A",
                TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "_text")),
            ),
            provider_type(
                "pg_catalog",
                "_text",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("pg_catalog", "text")),
            ),
            provider_type(
                "pg_catalog",
                "_int8",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("pg_catalog", "int8")),
            ),
            provider_type(
                "public",
                "_label_domain",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("public", "label_domain")),
            ),
        ],
    }
    .to_catalog()
    .expect("structured type catalog builds")
}

fn native_enum_catalog() -> Catalog {
    let enumeration = |schema: &str, name: &str, description: &str| TypeMetadata {
        internal_type: name.to_string(),
        readable_type: format!("{schema}.{name}"),
        schema: schema.to_string(),
        structure: TypeStructureMetadata::enumeration(EnumTypeMetadata {
            description: Some(description.to_string()),
            variants: ["pending", "active", "archived"]
                .into_iter()
                .map(|variant| EnumVariantMetadata {
                    variant: variant.to_string(),
                    database_value: variant.to_string(),
                    label: None,
                    description: None,
                })
                .collect(),
        }),
        provider: Some(ProviderTypeFacts {
            kind: "e".to_string(),
            category: "E".to_string(),
            effective_kind: Some("e".to_string()),
            effective_category: Some("E".to_string()),
            orderable: true,
        }),
        operations: ["=", "<>", "<", "<=", ">", ">="]
            .into_iter()
            .map(str::to_string)
            .collect(),
    };
    let provider_type =
        |name: &str, kind: &str, category: &str, structure: TypeStructureMetadata| TypeMetadata {
            internal_type: name.to_string(),
            readable_type: format!("public.{name}"),
            schema: "public".to_string(),
            structure,
            provider: Some(ProviderTypeFacts {
                kind: kind.to_string(),
                category: category.to_string(),
                effective_kind: None,
                effective_category: None,
                orderable: true,
            }),
            operations: ["=", "<>"].into_iter().map(str::to_string).collect(),
        };
    let column = |name: &str, provider_type: TypeKey| ColumnMetadata {
        name: name.to_string(),
        description: None,
        database_type: provider_type.name.clone(),
        provider_type,
        formatted_type: None,
        type_modifier: None,
        data_type: DataType::Unknown,
        not_null: true,
    };
    let status = TypeKey::new("public", "status");
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![TableMetadata {
                schema: "public".to_string(),
                name: "enum_records".to_string(),
                object_type: ObjectType::Table,
                description: None,
                columns: vec![
                    column("status", status.clone()),
                    column("domain_status", TypeKey::new("public", "status_domain")),
                    column("statuses", TypeKey::new("public", "_status")),
                ],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: vec![
            enumeration("public", "status", "Lifecycle status."),
            provider_type(
                "status_domain",
                "d",
                "E",
                TypeStructureMetadata::domain(status.clone()),
            ),
            provider_type("_status", "b", "A", TypeStructureMetadata::array(status)),
        ],
    }
    .to_catalog()
    .expect("native enum generation catalog builds")
}
const MEMBERSHIPS_SCHEMA: &str = r#"---
schema: public
name: memberships
object_type: table
columns:
  - name: tenant_id
    provider_type:
      schema: pg_catalog
      name: uuid
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: user_id
    provider_type:
      schema: pg_catalog
      name: uuid
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: locale
    provider_type:
      schema: pg_catalog
      name: text
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
    access_method: btree
    keys:
      - column: tenant_id
      - column: user_id
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
    .to_catalog()
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

async fn set_document_text(bowl: &Bowl, path: &str, text: &str) {
    let files = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let file = files
        .collect()
        .into_iter()
        .find_map(|(entity, candidate)| (candidate.0 == path).then_some(entity))
        .expect("source document exists");
    let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
    let source = sources
        .collect()
        .into_iter()
        .find_map(|(entity, source)| (entity == file).then_some(source))
        .expect("source text exists");
    let text = text.to_string();
    source
        .with_latest(move |source| source.set_text(&text))
        .await;
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

fn write_test_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("test file parent");
    }
    std::fs::write(path, contents).expect("test file writes");
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
async fn unresolved_spreads_block_artifacts_and_restore_cleanly() {
    const QUERY: &str = "query Titles {\n  title(limit 1) {\n    ...TitleBits\n  }\n}\n";
    const OTHER: &str = "fragment OtherBits on title {\n  id\n}\n";
    const TARGET: &str = "fragment TitleBits on title {\n  id\n}\n";

    let bowl = memory_bowl(
        scoped_catalog(),
        vec![
            document("queries/frontend/query.dsql", QUERY, "frontend"),
            document("queries/frontend/fragments.dsql", OTHER, "frontend"),
        ],
        BTreeMap::new(),
    )
    .await;

    let missing = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("an unresolved spread must block artifact assembly");
    assert!(
        missing.to_string().contains("TitleBits"),
        "unexpected assembly error: {missing}"
    );

    set_document_text(&bowl, "queries/frontend/fragments.dsql", TARGET).await;

    let resolved = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("the supplied fragment permits artifact assembly");
    let operation = resolved
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "Titles")
        .expect("resolved query emits an operation")
        .serialized
        .clone();

    set_document_text(&bowl, "queries/frontend/fragments.dsql", OTHER).await;
    assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("removing the fragment must block artifact assembly again");

    set_document_text(&bowl, "queries/frontend/fragments.dsql", TARGET).await;
    let restored = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("restoring the fragment permits artifact assembly again");
    let restored_operation = restored
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "Titles")
        .expect("restored query emits an operation");
    assert_eq!(restored_operation.serialized, operation);
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
                context { allowed: bool }
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
                query NumericMetrics(%minimum = 12345678901234567890.12345678901234567890) {
                  metrics(where .amount >= %minimum and .amount in %amounts) { amount ratio }
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
async fn provider_scalar_wire_identity_flows_through_generated_metadata() {
    let bowl = memory_bowl(
        provider_scalar_catalog(),
        vec![document(
            "queries/frontend/events.dsql",
            "query Events { events(where .event_date == %date and .big_id >= %minimum_big_id) { event_date big_id } }",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let artifact = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("assembly succeeds")
        .snapshot
        .artifacts
        .into_iter()
        .find(|artifact| artifact.name == "Events")
        .expect("provider scalar operation artifact");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn configured_catalog_types_flow_through_generated_metadata() {
    let scratch = tempfile::tempdir().expect("configured-type scratch directory");
    let dir = scratch.path();
    write_test_file(
        dir,
        "dsql/dsql.toml",
        indoc::indoc! {r#"
            database_url = "x"

            [[catalog.types]]
            pg = "pg_catalog.date"
            name = "Date"
            wire = "text"
            literal = "string"
            pattern = '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'
            operators = ["eq", "ne"]
            orderable = false
        "#},
    );
    dsql_project::store_metadata_dir(&provider_scalar_metadata(), &dir.join("dsql/schema"))
        .await
        .expect("configured-type metadata stores");
    write_test_file(
        dir,
        "dsql/queries/events.dsql",
        indoc::indoc! {r#"
            query MappedEvents(%date = "2026-07-27") {
              events(where .event_date == %date) {
                event_date
                event_dates
              }
            }

            query DynamicMappedEvents(%search = {}) {
              events(where %search on selected) {
                event_date
              }
            }
        "#},
    );

    let project = Project::load_from(dir)
        .await
        .expect("configured-type project loads");
    let output = generate_project(&project, GenerateOptions::default(), MatchLockMode::Update)
        .await
        .expect("configured-type project generates");
    let manifest = std::fs::read_to_string(&output.manifest_path).expect("manifest reads");
    let manifest: dsql_metadata::BuildManifest =
        facet_json::from_str(&manifest).expect("manifest parses");
    let entry = manifest
        .operations
        .iter()
        .find(|entry| entry.name == "MappedEvents")
        .expect("mapped operation is published");
    let artifact = std::fs::read_to_string(project.root.join("build").join(&entry.path))
        .expect("mapped operation artifact reads");
    let dynamic_entry = manifest
        .operations
        .iter()
        .find(|entry| entry.name == "DynamicMappedEvents")
        .expect("mapped dynamic operation is published");
    let dynamic_artifact =
        std::fs::read_to_string(project.root.join("build").join(&dynamic_entry.path))
            .expect("mapped dynamic operation artifact reads");

    insta::assert_snapshot!(artifact);
    insta::assert_snapshot!("configured_catalog_type_dynamic_input", dynamic_artifact);
    write_test_file(
        dir,
        "dsql/queries/events.dsql",
        "query MappedEvents(%date = \"tomorrow\") { events(where .event_date == %date) { event_date } }",
    );
    let error = generate_project(&project, GenerateOptions::default(), MatchLockMode::Update)
        .await
        .expect_err("mapped pattern rejects an invalid declaration default");
    insta::assert_snapshot!(
        "configured_catalog_type_rejects_bad_default",
        error
            .to_string()
            .replace(&dir.display().to_string(), "<project>")
    );
    write_test_file(
        dir,
        "dsql/queries/events.dsql",
        "query MappedEvents { events(where .event_date == \"tomorrow\") { event_date } }",
    );
    let error = generate_project(&project, GenerateOptions::default(), MatchLockMode::Update)
        .await
        .expect_err("mapped pattern rejects an invalid literal");
    insta::assert_snapshot!(
        "configured_catalog_type_rejects_bad_literal",
        error
            .to_string()
            .replace(&dir.display().to_string(), "<project>")
    );
}

#[tokio::test]
async fn provider_scalar_dynamic_inputs_use_the_effective_logical_name() {
    let bowl = memory_bowl(
        provider_scalar_catalog(),
        vec![document(
            "queries/frontend/events-dynamic.dsql",
            "query DynamicEvents(%search = {}) { events(where %search on selected) { event_date } }",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let artifact = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("assembly succeeds")
        .snapshot
        .artifacts
        .into_iter()
        .find(|artifact| artifact.name == "DynamicEvents")
        .expect("dynamic provider operation artifact");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn provider_array_dynamic_inputs_remain_unsupported() {
    let bowl = memory_bowl(
        provider_scalar_catalog(),
        vec![document(
            "queries/frontend/events-array-dynamic.dsql",
            "query DynamicEventArrays(%search = {}) { events(where %search on selected) { event_dates } }",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("database-array dynamic operands stay unsupported");

    insta::assert_snapshot!(error);
}

#[tokio::test]
async fn structured_result_values_flow_through_generated_metadata() {
    let bowl = memory_bowl(
        structured_type_catalog(),
        vec![document(
            "queries/frontend/structured.dsql",
            "query Structured(%label = \"primary\" %address = \"127.0.0.1\") { typed_values(where .label == %label and .address == %address limit 1) { label address labels big_values nested_label domain_labels labeled_values } }",
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let artifact = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("assembly succeeds")
        .snapshot
        .artifacts
        .into_iter()
        .find(|artifact| artifact.name == "Structured")
        .expect("structured operation artifact");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn native_enum_contracts_flow_through_generated_metadata() {
    let bowl = memory_bowl(
        native_enum_catalog(),
        vec![document(
            "queries/frontend/native-enums.dsql",
            indoc::indoc! {r#"
                context { status: public::status }
                filter EnumContext on enum_records {
                  where .status == $:status
                }
                query NativeEnums(
                  %status = "active"
                  %statuses = ["pending", "archived"]
                ) {
                  enum_records(
                    filter EnumContext
                    where .status == %status
                      and .status in %statuses
                    limit 1
                  ) {
                    status
                    domain_status
                    statuses
                  }
                }
                query DynamicEnums(%search = {}) {
                  enum_records(where %search on selected) {
                    status
                  }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let mut artifacts = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect("native enum assembly succeeds")
        .snapshot
        .artifacts
        .into_iter()
        .filter(|artifact| matches!(artifact.name.as_str(), "DynamicEnums" | "NativeEnums"))
        .map(|artifact| format!("{}\n{}", artifact.name, artifact.serialized))
        .collect::<Vec<_>>();
    artifacts.sort();

    insta::assert_snapshot!(artifacts.join("\n---\n"));
}

#[tokio::test]
async fn provider_policy_context_mismatches_block_generation() {
    let bowl = memory_bowl(
        provider_scalar_catalog(),
        vec![document(
            "queries/frontend/policy.dsql",
            indoc::indoc! {r#"
                context { shared: pg_catalog::date }
                filter Mixed on events {
                  where .event_date == $:shared and .address == $:shared
                }
                query Events { events { event_date } }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;

    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("an invalid compiled policy must block generation");

    insta::assert_snapshot!(error.to_string());
}

#[tokio::test]
async fn bounded_dynamic_inputs_flow_through_sql_and_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/dynamic.dsql",
            indoc::indoc! {r#"
                context { is_admin: bool }
                filter NameAccess on public::users {
                  apply where true
                  field name where $:is_admin
                }
                query DynamicUsers(
                  %selected_search = {}
                  %indexed_search = {}
                  %searchable_search = {}
                  %selected_order = []
                  %indexed_order = []
                  %selected_indexed_order = []
                  %searchable_order = []
                  %aggregate_search = {}
                  %aggregate_indexed_search = {}
                ) {
                  users(
                    where %selected_search on selected
                      and %indexed_search on indexed
                      and %searchable_search on searchable
                    order by name asc,
                      %selected_order on selected,
                      %indexed_order on indexed,
                      %selected_indexed_order on selected_indexed,
                      %searchable_order on searchable,
                      id desc
                  ) {
                    id
                    label: name
                  }
                  summary: users(
                    where %aggregate_search on searchable
                      and %aggregate_indexed_search on indexed
                  ) | aggregate {
                    count
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
    let artifact = assembled
        .snapshot
        .artifacts
        .iter()
        .find(|artifact| artifact.name == "DynamicUsers")
        .expect("dynamic operation artifact");

    insta::assert_snapshot!(artifact.serialized);
}

#[tokio::test]
async fn reused_dynamic_inputs_require_identical_selected_surfaces() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/incompatible-dynamic.dsql",
            indoc::indoc! {r#"
                query IncompatibleDynamic(%search = {}) {
                  by_id: users(where %search on selected) { id }
                  by_name: users(where %search on selected) { name }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("incompatible dynamic surfaces must fail generation");
    let message = error.to_string();
    assert!(
        message.contains("params.search")
            && message.contains("expanded capability surface is incompatible"),
        "unexpected dynamic surface conflict: {message}",
    );
}

#[tokio::test]
async fn reused_dynamic_inputs_require_identical_preset_spellings() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/incompatible-preset.dsql",
            indoc::indoc! {r#"
                query IncompatiblePreset(%search = {}) {
                  selected: users(where %search on selected_indexed) { id name }
                  catalog: users(where %search on indexed) { id name }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("one dynamic input must use one preset spelling");

    insta::assert_snapshot!(error);
}

#[tokio::test]
async fn dynamic_predicates_reject_selected_aliases_reserved_for_composition() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA]),
        vec![document(
            "queries/frontend/reserved-dynamic.dsql",
            indoc::indoc! {r#"
                query ReservedDynamic(%search = {}) {
                  users(where %search on selected) { not: name }
                }
            "#},
            "frontend",
        )],
        BTreeMap::new(),
    )
    .await;
    let error = assemble_bowl(&bowl, None, GenerateOptions::default())
        .await
        .expect_err("reserved predicate composition keys must fail generation");
    let message = error.to_string();
    assert!(
        message.contains("selected field `not`") && message.contains("alias the selected field"),
        "unexpected reserved dynamic field error: {message}",
    );
}

#[tokio::test]
async fn defaults_and_fragment_lifting_flow_through_operation_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/variable-contracts.dsql",
            indoc::indoc! {r#"
                fragment PostWindow(%after? = null %limit = 5) on users {
                  posts(where .created_at >= %after limit %) { id }
                }
                query Contained { users { ...PostWindow } }
                query Bound(%since = "2020-01-01T00:00:00Z") {
                  users { ...PostWindow(%after <- %since) }
                }
                query NullableCollection(%ids? = null) {
                  users(where .id in %ids) { id }
                }
                query CollectionDefault(
                  %ids = ["00000000-0000-0000-0000-000000000001"]
                ) {
                  users(where .id in %ids) { id }
                }
                fragment ParentWindow on users {
                  ...PostWindow(%)
                }
                query DeepContained {
                  users { ...ParentWindow }
                }
                query NullableNonNullDefault(%limit? = 5) {
                  users(limit %limit) { id }
                }
                query ClosedVariants(
                  %operator = "=="
                  %direction = "asc"
                ) {
                  users(
                    where .name %operator[==, like] "A"
                    order by name %direction
                  ) { id }
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
                "PostWindow"
                    | "Contained"
                    | "Bound"
                    | "ClosedVariants"
                    | "NullableCollection"
                    | "CollectionDefault"
                    | "ParentWindow"
                    | "DeepContained"
                    | "NullableNonNullDefault"
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
                context {
                  user_id: uuid
                  unused_context: bool
                }
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
async fn field_filter_context_metadata_follows_reached_guards() {
    const FILTERS: &str = indoc::indoc! {r#"
        context {
          name_guard: text
          id_guard: uuid
        }
        filter NameAccess on users {
          apply
          field name where .name == $:name_guard
        }
        filter IdAccess on users {
          apply
          field id where .id == $:id_guard
        }
    "#};

    async fn operation_context(catalog: Catalog, source: String, operation: &str) -> Vec<String> {
        let bowl = memory_bowl(
            catalog,
            vec![document(
                "queries/frontend/guards.dsql",
                &source,
                "frontend",
            )],
            BTreeMap::new(),
        )
        .await;
        let assembled = assemble_bowl(&bowl, None, GenerateOptions::default())
            .await
            .expect("field guard metadata assembles");
        let artifact = assembled
            .snapshot
            .artifacts
            .iter()
            .find(|artifact| artifact.name == operation)
            .expect("operation artifact exists");
        let metadata: dsql_metadata::OperationMetadata =
            facet_json::from_str(&artifact.serialized).expect("operation metadata parses");
        metadata
            .context
            .into_iter()
            .map(|field| field.path)
            .collect()
    }

    let unused = operation_context(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        format!("{FILTERS}query Unused {{ users(limit 1) {{ posts {{ title }} }} }}\n"),
        "Unused",
    )
    .await;
    let one = operation_context(
        catalog_from_tables([USERS_SCHEMA]),
        format!("{FILTERS}query One {{ users(limit 1) {{ name }} }}\n"),
        "One",
    )
    .await;
    let both = operation_context(
        catalog_from_tables([USERS_SCHEMA]),
        format!("{FILTERS}query Both {{ users(limit 1) {{ id name }} }}\n"),
        "Both",
    )
    .await;

    assert!(unused.is_empty());
    assert_eq!(one, ["context.name_guard"]);
    assert_eq!(both, ["context.id_guard", "context.name_guard"]);
}

#[tokio::test]
async fn policy_metadata_explains_composed_access_and_disabled_assignments() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/access.dsql",
            indoc::indoc! {r#"
                context {
                  is_admin: bool
                  user_id: uuid
                  can_read_posts: bool
                }
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
                query ByKey { users(where .id == %id) { id name } }
                query ByCompositeKey {
                  memberships(where .tenant_id == %tenant and .user_id == %user) { locale }
                }
                query RuntimeLimit { users(limit %count) { id } }
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
        .filter(|artifact| artifact.kind == DefinitionKind::Query)
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
                    context { can_read_users: bool }
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
async fn aggregate_objects_flow_through_operation_and_fragment_metadata() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/aggregates.dsql",
            indoc::indoc! {r#"
                fragment UserStats on users {
                  post_stats: posts(where .title == %title) | aggregate {
                    count
                    latest: max .created_at
                  }
                  post_groups: posts | aggregate by title_group: .title {
                    count
                    latest_group: max .created_at
                  }
                }
                fragment FlatPostStats on users {
                  ...posts(where .title == %flat_title) | aggregate {
                    flat_post_count: count
                    flat_latest: max .created_at
                  }
                }
                query RootStats {
                  user_stats: users(where .name == %name) | aggregate {
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
                  ...users(where .name == %flat_name) | aggregate {
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
                    ...users(where .name == %owner_name) {
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

#[tokio::test]
async fn multiple_roots_assemble_as_one_operation_contract() {
    let bowl = memory_bowl(
        catalog_from_tables([USERS_SCHEMA, POSTS_SCHEMA]),
        vec![document(
            "queries/frontend/multiple-roots.dsql",
            indoc::indoc! {r#"
                fragment UserBits on users {
                  name
                }
                query UserOverview(%name? = null) {
                  summary: users(where .name == %name) | aggregate {
                    count
                  }
                  first: users(where .name == %name limit 1) {
                    id
                    ...UserBits
                  }
                  ...posts | aggregate {
                    post_count: count
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
        .expect("multiple roots assemble");
    let operations = assembled
        .snapshot
        .artifacts
        .iter()
        .filter(|artifact| artifact.family == dsql_generate::ArtifactFamily::Operation)
        .collect::<Vec<_>>();
    assert_eq!(operations.len(), 1, "the definition emits one operation");
    assert_eq!(operations[0].name, "UserOverview");

    insta::assert_snapshot!(operations[0].serialized);
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
