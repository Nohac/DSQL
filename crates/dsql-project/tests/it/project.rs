//! Loads the imdb fixture project into a bowl and drives it.

use std::collections::BTreeSet;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use bowl::{Entity, Query};
use dsql_core::catalog::{
    ColumnMetadata, DataType, DatabaseMetadata, ForeignKeyConstraintMetadata, ForeignKeyDirection,
    ForeignKeyReferenceMetadata, IndexKeyMetadata, IndexMetadata, ObjectType, RelationCardinality,
    SchemaMetadata, TableConstraintKind, TableConstraintMetadata, TableMetadata, TypeMetadata,
};
use dsql_core::facts::{Diagnostic, Severity, arm_generate_demands};
use dsql_core::source::SourceKind;
use dsql_core::sql::GeneratedSqlFact;
use dsql_project::{Project, ProjectError, find_root, open_project_bowl};
use tempfile::TempDir;

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/it/fixture/{name}"))
}

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent directory");
    }
    std::fs::write(path, contents).expect("fixture file");
}

fn scratch() -> TempDir {
    tempfile::tempdir().expect("scratch directory")
}

fn scratch_project(config: &str) -> TempDir {
    let scratch = scratch();
    write_file(scratch.path(), "dsql/dsql.toml", config);
    scratch
}

async fn store_overlay_catalog(root: &Path) {
    let metadata = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![
                TableMetadata {
                    schema: "public".to_string(),
                    name: "accounts".to_string(),
                    object_type: ObjectType::Table,
                    description: None,
                    columns: vec![
                        ColumnMetadata {
                            name: "id".to_string(),
                            description: None,
                            database_type: "uuid".to_string(),
                            data_type: DataType::Uuid,
                            not_null: true,
                        },
                        ColumnMetadata {
                            name: "secret".to_string(),
                            description: None,
                            database_type: "text".to_string(),
                            data_type: DataType::Text,
                            not_null: false,
                        },
                        ColumnMetadata {
                            name: "manager_id".to_string(),
                            description: None,
                            database_type: "uuid".to_string(),
                            data_type: DataType::Uuid,
                            not_null: true,
                        },
                    ],
                    constraints: vec![TableConstraintMetadata {
                        name: Some("accounts_pkey".to_string()),
                        kind: TableConstraintKind::PrimaryKey,
                        columns: vec!["id".to_string()],
                    }],
                    foreign_keys: vec![ForeignKeyConstraintMetadata {
                        name: Some("accounts_manager_id_fkey".to_string()),
                        columns: vec!["manager_id".to_string()],
                        references: ForeignKeyReferenceMetadata {
                            schema: "public".to_string(),
                            table: "accounts".to_string(),
                            columns: vec!["id".to_string()],
                        },
                    }],
                    indexes: Vec::new(),
                },
                TableMetadata {
                    schema: "public".to_string(),
                    name: "account_summary".to_string(),
                    object_type: ObjectType::View,
                    description: None,
                    columns: vec![
                        ColumnMetadata {
                            name: "account_id".to_string(),
                            description: None,
                            database_type: "uuid".to_string(),
                            data_type: DataType::Uuid,
                            not_null: true,
                        },
                        ColumnMetadata {
                            name: "label".to_string(),
                            description: None,
                            database_type: "text".to_string(),
                            data_type: DataType::Text,
                            not_null: true,
                        },
                    ],
                    constraints: Vec::new(),
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                },
            ],
        }],
        types: ["uuid", "text"]
            .into_iter()
            .map(|data_type| TypeMetadata {
                internal_type: data_type.to_string(),
                readable_type: data_type.to_string(),
                schema: "pg_catalog".to_string(),
                operations: BTreeSet::new(),
            })
            .collect(),
    };
    dsql_project::store_metadata_dir(&metadata, &root.join("dsql/schema"))
        .await
        .expect("overlay fixture catalog stores");
}

#[tokio::test]
async fn catalog_overlays_compose_visibility_documentation_and_view_relationships() {
    let scratch = scratch_project("database_url = \"x\"\n");
    store_overlay_catalog(scratch.path()).await;
    let project = Project::load_from(scratch.path())
        .await
        .expect("project loads before overlays");
    let provider_fingerprint = project
        .load_catalog()
        .await
        .expect("provider catalog loads")
        .semantic_fingerprint();
    write_file(
        scratch.path(),
        "dsql/overlays/read-models.yaml",
        indoc::indoc! {r#"
            version: 1
            objects:
              - target:
                  schema: public
                  name: accounts
                description: Public account records.
                columns:
                  - name: secret
                    hidden: true
                relationships:
                  - name: manager
                    target:
                      schema: public
                      name: accounts
                    columns:
                      - local: manager_id
                        target: id
                  - name: summary
                    target:
                      schema: public
                      name: account_summary
                    columns:
                      - local: id
                        target: account_id
                hide:
                  relationships:
                    - target:
                        schema: public
                        name: accounts
                      selector: manager_id
                      direction: referencing
              - target:
                  schema: public
                  name: account_summary
                assert_unique:
                  - name: account_summary_account_id_unique
                    columns: [account_id]
        "#},
    );
    write_file(
        scratch.path(),
        "dsql/read-model.dsql",
        indoc::indoc! {r#"
            query AccountSummary {
              accounts {
                id
                summary {
                  label
                }
              }
            }
        "#},
    );

    let catalog = project.load_catalog().await.expect("overlay composes");
    assert_ne!(
        catalog.semantic_fingerprint(),
        provider_fingerprint,
        "overlay-only semantic changes move the effective fingerprint"
    );
    let accounts = catalog
        .table("public", "accounts")
        .expect("accounts remains visible");
    assert_eq!(
        accounts.description.as_deref(),
        Some("Public account records.")
    );
    assert!(
        catalog
            .columns_for_table(accounts.id)
            .all(|column| column.name != "secret"),
        "hidden columns leave the query-facing catalog"
    );
    let manager = catalog
        .relation_fields_for_table(accounts.id)
        .into_iter()
        .find(|relation| relation.name == "manager")
        .expect("authored replacement relationship is exposed");
    assert_eq!(manager.relation.cardinality, RelationCardinality::Singular);
    assert!(
        !manager.relation.nullable,
        "a forward non-null provider foreign key proves presence"
    );
    assert!(manager.relation.join_support.is_some());
    assert!(
        manager
            .relation
            .supports
            .declaration
            .as_ref()
            .is_some_and(|support| support.path.ends_with("read-models.yaml"))
    );
    assert!(
        manager
            .relation
            .supports
            .join
            .iter()
            .any(|support| support.path.ends_with("accounts.yaml"))
    );
    let visible_provider_directions = catalog
        .relation_fields_for_table(accounts.id)
        .into_iter()
        .filter(|relation| relation.name == "accounts")
        .collect::<Vec<_>>();
    assert_eq!(
        visible_provider_directions.len(),
        1,
        "the directional self-edge hide leaves only the reverse relation"
    );
    assert_eq!(
        visible_provider_directions[0].relation.cardinality,
        RelationCardinality::Collection
    );
    let relation = catalog
        .relation_fields_for_table(accounts.id)
        .into_iter()
        .find(|relation| relation.name == "summary")
        .expect("authored relationship is exposed");
    assert_eq!(relation.relation.cardinality, RelationCardinality::Singular);
    assert!(
        relation.relation.nullable,
        "overlay uniqueness proves at-most-one but not existence"
    );
    assert!(
        relation
            .relation
            .supports
            .declaration
            .as_ref()
            .is_some_and(|support| support.path.ends_with("read-models.yaml")),
        "authored provenance is retained"
    );

    let bowl = open_project_bowl(&project)
        .await
        .expect("effective catalog opens");
    arm_generate_demands(&bowl).await;
    let diagnostics = bowl
        .scoop::<Query<(Entity, &Severity, &Diagnostic)>>()
        .await;
    let errors = diagnostics
        .collect()
        .into_iter()
        .filter(|(_, severity, _)| **severity == Severity::Error)
        .map(|(_, _, diagnostic)| diagnostic.0.clone())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "authored view relationship resolves cleanly: {errors:?}"
    );
    let generated = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let sql = generated
        .collect()
        .into_iter()
        .next()
        .map(|(_, generated)| generated.0.sql.clone())
        .expect("query generates SQL");
    insta::assert_snapshot!(sql);
}

#[tokio::test]
async fn catalog_overlay_fingerprint_tracks_semantics_not_yaml_order() {
    let scratch = scratch_project("database_url = \"x\"\n");
    store_overlay_catalog(scratch.path()).await;
    let overlay = "dsql/overlays/read-models/catalog.yaml";
    write_file(
        scratch.path(),
        overlay,
        indoc::indoc! {r#"
            version: 1
            objects:
              - target: { schema: public, name: accounts }
                description: Public accounts.
                columns:
                  - name: secret
                    hidden: true
              - target: { schema: public, name: account_summary }
                assert_unique:
                  - name: account_summary_account_id_unique
                    columns: [account_id]
        "#},
    );
    let project = Project::load_from(scratch.path())
        .await
        .expect("project loads");
    let initial = project
        .load_catalog()
        .await
        .expect("first overlay composes")
        .semantic_fingerprint();

    write_file(
        scratch.path(),
        overlay,
        indoc::indoc! {r#"
            version: 1
            objects:
              - assert_unique:
                  - columns:
                      - account_id
                    name: account_summary_account_id_unique
                target:
                  name: account_summary
                  schema: public
              - columns:
                  - hidden: true
                    name: secret
                target:
                  name: accounts
                  schema: public
                description: Public accounts.
        "#},
    );
    let reordered = project
        .load_catalog()
        .await
        .expect("reordered overlay composes")
        .semantic_fingerprint();
    assert_eq!(
        reordered, initial,
        "formatting and declaration order are fingerprint-neutral"
    );

    let changed = std::fs::read_to_string(scratch.path().join(overlay))
        .expect("overlay reads")
        .replace("Public accounts.", "Customer accounts.");
    write_file(scratch.path(), overlay, &changed);
    let changed = project
        .load_catalog()
        .await
        .expect("changed overlay composes")
        .semantic_fingerprint();
    assert_ne!(
        changed, initial,
        "consumer-visible overlay semantics move the fingerprint"
    );
}

#[tokio::test]
async fn catalog_overlays_hide_objects_and_individual_self_fk_directions() {
    let scratch = scratch_project("database_url = \"x\"\n");
    store_overlay_catalog(scratch.path()).await;
    write_file(
        scratch.path(),
        "dsql/overlays/visibility.yaml",
        indoc::indoc! {r#"
            version: 1
            objects:
              - target: { schema: public, name: accounts }
                hide:
                  relationships:
                    - target: { schema: public, name: accounts }
                      selector: manager_id
                      direction: referenced
              - target: { schema: public, name: account_summary }
                hidden: true
        "#},
    );
    let project = Project::load_from(scratch.path())
        .await
        .expect("configuration loads");
    let catalog = project.load_catalog().await.expect("visibility composes");
    assert!(
        catalog.table("public", "account_summary").is_none(),
        "whole-object hides remove roots from the query-facing catalog"
    );
    let accounts = catalog
        .table("public", "accounts")
        .expect("visible object remains");
    let provider_directions = catalog
        .relation_fields_for_table(accounts.id)
        .into_iter()
        .filter(|relation| relation.name == "accounts")
        .collect::<Vec<_>>();
    assert_eq!(
        provider_directions.len(),
        1,
        "the referenced self-FK direction alone is hidden"
    );
    assert_eq!(
        provider_directions[0].relation.join_direction,
        Some(ForeignKeyDirection::Referencing)
    );
    assert_eq!(
        provider_directions[0].relation.cardinality,
        RelationCardinality::Singular
    );
    assert!(!provider_directions[0].relation.nullable);
}

#[tokio::test]
async fn catalog_overlays_reject_table_uniqueness_assertions() {
    let scratch = scratch_project("database_url = \"x\"\n");
    store_overlay_catalog(scratch.path()).await;
    write_file(
        scratch.path(),
        "dsql/overlays/invalid.yaml",
        indoc::indoc! {r#"
            version: 1
            objects:
              - target:
                  schema: public
                  name: accounts
                assert_unique:
                  - name: unsafe_account_unique
                    columns: [secret]
        "#},
    );

    let project = Project::load_from(scratch.path())
        .await
        .expect("configuration loads");
    let error = project
        .load_catalog()
        .await
        .expect_err("table assertions fail closed");
    insta::assert_snapshot!(
        error
            .to_string()
            .replace(&scratch.path().display().to_string(), "<project>")
    );
}

#[tokio::test]
async fn catalog_overlays_reject_unknown_keys_and_redundant_unique_indexes() {
    let unknown = scratch_project("database_url = \"x\"\n");
    store_overlay_catalog(unknown.path()).await;
    write_file(
        unknown.path(),
        "dsql/overlays/unknown.yaml",
        indoc::indoc! {r#"
            version: 1
            objects:
              - target: { schema: public, name: accounts }
                columns:
                  - name: secret
                    hiddn: true
        "#},
    );
    let project = Project::load_from(unknown.path())
        .await
        .expect("configuration loads");
    let unknown_error = project
        .load_catalog()
        .await
        .expect_err("unknown nested overlay keys fail closed")
        .to_string()
        .replace(&unknown.path().display().to_string(), "<project>");

    let redundant = scratch_project("database_url = \"x\"\n");
    store_overlay_catalog(redundant.path()).await;
    let schema = redundant.path().join("dsql/schema");
    let mut metadata = dsql_project::load_metadata_dir(&schema)
        .await
        .expect("provider metadata loads");
    let summary = metadata
        .schemas
        .iter_mut()
        .flat_map(|schema| &mut schema.tables)
        .find(|table| table.name == "account_summary")
        .expect("summary view");
    summary.object_type = ObjectType::MaterializedView;
    summary.indexes.push(IndexMetadata {
        name: Some("account_summary_account_id_idx".to_string()),
        access_method: "btree".to_string(),
        keys: vec![IndexKeyMetadata {
            column: "account_id".to_string(),
            operator_class: None,
            capabilities: Vec::new(),
            order: None,
        }],
        included_columns: vec!["label".to_string()],
        unique: true,
    });
    dsql_project::store_metadata_dir(&metadata, &schema)
        .await
        .expect("provider unique index publishes");
    write_file(
        redundant.path(),
        "dsql/overlays/redundant.yaml",
        indoc::indoc! {r#"
            version: 1
            objects:
              - target: { schema: public, name: account_summary }
                assert_unique:
                  - name: authored_account_id_unique
                    columns: [account_id]
        "#},
    );
    let project = Project::load_from(redundant.path())
        .await
        .expect("configuration loads");
    let redundant_error = project
        .load_catalog()
        .await
        .expect_err("provider unique indexes make equal assertions redundant")
        .to_string()
        .replace(&redundant.path().display().to_string(), "<project>");

    insta::assert_snapshot!(format!(
        "unknown key: {unknown_error}\nredundant unique index: {redundant_error}"
    ));
}

#[tokio::test]
async fn catalog_overlay_validation_reports_conflicts_and_stale_references() {
    let cases = [
        (
            "missing column",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    columns:
                      - name: renamed
                        hidden: true
            "#},
            None,
        ),
        (
            "incompatible mapping",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    relationships:
                      - name: broken
                        target: { schema: public, name: account_summary }
                        columns:
                          - { local: id, target: label }
            "#},
            None,
        ),
        (
            "invalid relationship name",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    relationships:
                      - name: not-selectable
                        target: { schema: public, name: account_summary }
                        columns:
                          - { local: id, target: account_id }
            "#},
            None,
        ),
        (
            "column relationship collision",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    relationships:
                      - name: secret
                        target: { schema: public, name: account_summary }
                        columns:
                          - { local: id, target: account_id }
            "#},
            None,
        ),
        (
            "provider relationship collision",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    relationships:
                      - name: accounts
                        target: { schema: public, name: account_summary }
                        columns:
                          - { local: id, target: account_id }
            "#},
            None,
        ),
        (
            "repeated uniqueness column",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: account_summary }
                    assert_unique:
                      - name: repeated
                        columns: [account_id, account_id]
            "#},
            None,
        ),
        (
            "hidden target",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: account_summary }
                    hidden: true
                  - target: { schema: public, name: accounts }
                    relationships:
                      - name: summary
                        target: { schema: public, name: account_summary }
                        columns:
                          - { local: id, target: account_id }
            "#},
            None,
        ),
        (
            "redundant hide to hidden target",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    hide:
                      relationships:
                        - target: { schema: public, name: accounts }
                          selector: manager_id
                          direction: referencing
                  - target: { schema: public, name: accounts }
                    hidden: true
            "#},
            None,
        ),
        (
            "duplicate ownership",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    description: First owner.
            "#},
            Some(indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    description: Second owner.
            "#}),
        ),
        (
            "missing provider edge",
            indoc::indoc! {r#"
                version: 1
                objects:
                  - target: { schema: public, name: accounts }
                    hide:
                      relationships:
                        - target: { schema: public, name: account_summary }
                          selector: account_id
                          direction: referenced
            "#},
            None,
        ),
    ];
    let mut rendered = Vec::new();
    for (name, first, second) in cases {
        let scratch = scratch_project("database_url = \"x\"\n");
        store_overlay_catalog(scratch.path()).await;
        write_file(scratch.path(), "dsql/overlays/a.yaml", first);
        if let Some(second) = second {
            write_file(scratch.path(), "dsql/overlays/b.yaml", second);
        }
        let project = Project::load_from(scratch.path())
            .await
            .expect("configuration loads");
        let error = project
            .load_catalog()
            .await
            .expect_err("invalid overlay fails closed")
            .to_string()
            .replace(&scratch.path().display().to_string(), "<project>");
        rendered.push(format!("{name}: {error}"));
    }
    insta::assert_snapshot!(rendered.join("\n"));
}

#[tokio::test]
async fn root_discovery_walks_upward() {
    let fixture = fixture_dir("imdb");
    let nested = fixture.join("dsql/queries");
    assert_eq!(find_root(&nested).await, Some(fixture.join("dsql")));
}

#[tokio::test]
async fn project_checks_clean_and_generates_sql() {
    let project = Project::load_from(&fixture_dir("imdb"))
        .await
        .expect("fixture project loads");
    assert_eq!(project.config.default_schema, "public");

    let bowl = open_project_bowl(&project).await.expect("bowl assembles");
    arm_generate_demands(&bowl).await;

    let diagnostics = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
    assert_eq!(diagnostics, 0, "fixture project must check clean");

    let generated = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let names: Vec<String> = generated
        .collect()
        .into_iter()
        .map(|(_, fact)| fact.0.operation_name.clone())
        .collect();
    assert_eq!(names, vec!["Titles".to_string()]);
}

#[tokio::test]
async fn scoped_project_resolves_imports_end_to_end() {
    let project = Project::load_from(&fixture_dir("scoped"))
        .await
        .expect("scoped fixture loads");
    assert!(
        matches!(
            project.config.lint.unindexed_scan_severity,
            Some(dsql_project::LintSeverity::Warning)
        ),
        "the [lint] section parses"
    );

    let bowl = open_project_bowl(&project).await.expect("bowl assembles");
    arm_generate_demands(&bowl).await;

    let diagnostics = bowl.scoop::<Query<(Entity, &Diagnostic)>>().await.len();
    assert_eq!(
        diagnostics, 0,
        "the frontend scope must see shared fragments"
    );

    let generated = bowl
        .scoop::<Query<(Entity, &GeneratedSqlFact)>>()
        .await
        .len();
    assert_eq!(
        generated, 2,
        "plain and embedded queries must plan and render"
    );
}

#[tokio::test]
async fn host_documents_load_whole() {
    let fixture = fixture_dir("scoped");
    let project = Project::load_from(&fixture)
        .await
        .expect("scoped fixture loads");

    // The loader does not extract or classify: hosts arrive whole; the
    // source model routes them at insert and the bowl derives regions.
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("documents load");
    let hosts: Vec<_> = documents
        .iter()
        .filter(|document| matches!(document.kind, SourceKind::Embedded(_)))
        .collect();
    assert_eq!(hosts.len(), 1, "the fixture has one host source");
    let host = hosts[0];
    assert_eq!(host.scope, "frontend");
    assert!(host.text.contains("import"), "hosts carry their full text");
    assert!(host.text.contains("query TitlePanel"));
}

#[tokio::test]
async fn configured_resolvers_drive_discovery_independently_of_extensions() {
    let scratch = scratch_project("database_url = \"x\"\n");
    let dir = scratch.path();
    write_file(dir, "dsql/plain.dsql", "query Plain { title { id } }\n");
    write_file(dir, "dsql/host.ts", "export const value = 1;\n");
    write_file(dir, "dsql/notes.md", "not a source\n");

    let project = Project::load_from(dir).await.expect("project loads");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("default documents load");
    let names: Vec<_> = documents
        .iter()
        .filter_map(|document| document.path.file_name().and_then(|name| name.to_str()))
        .collect();
    assert_eq!(
        names,
        vec!["plain.dsql"],
        "default discovery is standalone-only"
    );

    write_file(dir, "sources/plain.query", "query Plain { title { id } }\n");
    write_file(
        dir,
        "sources/host.component",
        "export const value = dsql`query Host { title { id } }`;\n",
    );
    write_file(dir, "sources/notes.md", "not a source\n");
    write_file(
        dir,
        "dsql/dsql.toml",
        "database_url = \"x\"\ndocuments = [\n  { resolver = \"dsql\", paths = [\"sources/plain.query\"] },\n  { resolver = \"typescript\", paths = [\"sources/host.component\"] },\n]\n",
    );

    let project = Project::load_from(dir)
        .await
        .expect("configured project loads");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("configured documents load");
    let classified: Vec<_> = documents
        .iter()
        .filter_map(|document| Some((document.path.file_name()?.to_str()?, document.kind.clone())))
        .collect();
    assert_eq!(
        classified,
        vec![
            (
                "host.component",
                SourceKind::Embedded("typescript".to_string())
            ),
            ("plain.query", SourceKind::Dsql),
        ],
        "configured discovery accepts exactly the shared source kinds"
    );

    // Named scopes replace the top-level default routing everywhere. An
    // output may therefore overlap the ignored group, while the effective
    // scope remains resolver-driven regardless of its file extension.
    write_file(
        dir,
        "dsql/dsql.toml",
        "database_url = \"x\"\ndocuments = [{ resolver = \"typescript\", paths = [\"ignored\"] }]\n\n[resolution.frontend]\ndocuments = [{ resolver = \"dsql\", paths = [\"sources/plain.query\"] }]\n\n[generate.typescript]\noutputs = [\"ignored\"]\n",
    );

    let project = Project::load_from(dir)
        .await
        .expect("ignored default routes do not reserve generator outputs");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("named documents load");
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].scope, "frontend");
    assert_eq!(documents[0].kind, SourceKind::Dsql);
    assert_eq!(
        documents[0].path.file_name().and_then(|name| name.to_str()),
        Some("plain.query")
    );
}

#[tokio::test]
async fn invalid_scope_import_graphs_are_rejected() {
    let cases = [
        (
            "unknown import",
            "database_url = \"x\"\n\n[resolution.frontend]\ndocuments = []\nimports = [\"missing\"]\n",
            "scope `frontend` imports unknown scope `missing`",
        ),
        (
            "cyclic imports",
            indoc::indoc! {r#"
                database_url = "x"

                [resolution.a]
                documents = []
                imports = ["b"]

                [resolution.b]
                documents = []
                imports = ["c"]

                [resolution.c]
                documents = []
                imports = ["a"]
            "#},
            "cyclic scope import: a -> b -> c -> a",
        ),
        (
            "duplicate import",
            indoc::indoc! {r#"
                database_url = "x"

                [resolution.shared]
                documents = [{ resolver = "dsql", paths = ["queries/**/*.dsql"] }]

                [resolution.frontend]
                documents = []
                imports = ["shared", "shared"]
            "#},
            "scope `frontend` imports scope `shared` more than once",
        ),
    ];
    for (case, config, expected) in cases {
        let scratch = scratch_project(config);
        let error = Project::load_from(scratch.path())
            .await
            .expect_err("invalid imports must fail");
        assert_eq!(error.to_string(), expected, "{case}");
    }
}

#[tokio::test]
async fn output_only_scopes_are_valid_but_empty_scopes_are_rejected() {
    let output = scratch_project(indoc::indoc! {r#"
        database_url = "x"

        [resolution.shared]
        documents = [{ resolver = "dsql", paths = ["queries/shared/**/*.dsql"] }]

        [resolution.shared_output]
        documents = []
        imports = ["shared"]
    "#});
    let project = Project::load_from(output.path())
        .await
        .expect("an importing output-only scope is valid");
    assert_eq!(
        project
            .config
            .scope_imports()
            .generation_targets()
            .collect::<Vec<_>>(),
        ["shared_output"]
    );

    let empty = scratch_project(indoc::indoc! {r#"
        database_url = "x"

        [resolution.empty]
        documents = []
    "#});
    let error = Project::load_from(empty.path())
        .await
        .expect_err("a scope without documents or imports is invalid");
    assert_eq!(
        error.to_string(),
        "scope `empty` has neither documents nor imports"
    );
}

#[tokio::test]
async fn duplicate_document_assignments_report_both_owners() {
    let cases = [
        (
            "two scopes",
            "database_url = \"x\"\n\n[resolution.a]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/source.any\"] }]\n\n[resolution.b]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/source.any\"] }]\n",
            ("a", "dsql", "b", "dsql"),
        ),
        (
            "two resolvers",
            "database_url = \"x\"\n\n[resolution.frontend]\ndocuments = [\n  { resolver = \"dsql\", paths = [\"queries/source.any\"] },\n  { resolver = \"typescript\", paths = [\"queries/source.any\"] },\n]\n",
            ("frontend", "dsql", "frontend", "typescript"),
        ),
    ];
    for (case, config, expected) in cases {
        let scratch = scratch_project(config);
        write_file(
            scratch.path(),
            "queries/source.any",
            "query Q { title { id } }\n",
        );
        let project = Project::load_from(scratch.path())
            .await
            .expect("config parses");
        let error = dsql_project::load_project_documents(&project)
            .await
            .expect_err("duplicate assignment must fail");
        let actual = if let ProjectError::DuplicateDocumentAssignment {
            path,
            first_scope,
            first_resolver,
            second_scope,
            second_resolver,
        } = error
        {
            Some((
                path,
                first_scope,
                first_resolver,
                second_scope,
                second_resolver,
            ))
        } else {
            None
        };
        assert_eq!(
            actual,
            Some((
                scratch.path().join("queries/source.any"),
                expected.0.to_string(),
                expected.1.to_string(),
                expected.2.to_string(),
                expected.3.to_string(),
            )),
            "{case}"
        );
    }
}

#[tokio::test]
async fn init_scaffolds_a_loadable_project() {
    let scratch = scratch();
    let dir = scratch.path();

    let project = dsql_project::init_project(dir, None)
        .await
        .expect("init scaffolds");
    assert!(project.schema.is_dir(), "schema/ directory exists");
    insta::assert_snapshot!(
        std::fs::read_to_string(project.root.join("dsql.toml")).expect("config written")
    );

    let reloaded = Project::load_from(dir).await.expect("project round-trips");
    assert_eq!(reloaded.config.database_url, "<database url>");
    assert_eq!(
        reloaded.config.resolution["main"].documents,
        vec![dsql_project::DocumentConfig {
            resolver: "dsql".to_string(),
            paths: vec!["**/*.dsql".to_string()],
        }]
    );

    // A second init must not clobber the existing configuration.
    let error = dsql_project::init_project(dir, Some("postgres://x".to_string()))
        .await
        .expect_err("re-init refuses");
    assert!(
        error.to_string().contains("already exists"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn init_rolls_back_when_the_schema_directory_is_blocked() {
    let scratch = scratch();
    let dir = scratch.path();
    // A regular file where schema/ must go blocks initialization.
    write_file(dir, "dsql/schema", "blocker");

    let error = dsql_project::init_project(dir, None)
        .await
        .expect_err("blocked schema fails init");
    assert!(
        error.to_string().contains("schema"),
        "the error names the failing path: {error}"
    );
    assert!(
        !dir.join("dsql/dsql.toml").exists(),
        "a failed init must not leave a config behind: {error}"
    );

    // Removing the blocker makes a retry succeed — nothing was stranded.
    std::fs::remove_file(dir.join("dsql/schema")).expect("unblock");
    dsql_project::init_project(dir, None)
        .await
        .expect("retry succeeds after unblocking");
}

#[tokio::test]
async fn init_escapes_hostile_database_urls() {
    // Quotes and backslashes are representable TOML: they MUST round-trip.
    let quoted_scratch = scratch();
    let dir = quoted_scratch.path();
    let quoted = "postgres://user:pa\"ss\\word@host/db".to_string();
    let project = dsql_project::init_project(dir, Some(quoted.clone()))
        .await
        .expect("representable URLs init");
    assert_eq!(project.config.database_url, quoted);
    let reloaded = Project::load_from(dir).await.expect("round-trips");
    assert_eq!(reloaded.config.database_url, quoted);

    // A newline may be unrepresentable in the starter: round-trip exactly
    // or fail cleanly — never write invalid TOML that strands the project.
    let hostile_scratch = scratch();
    let dir = hostile_scratch.path();
    let hostile = "postgres://user:pa\"ss\\wo\nrd@host/db".to_string();
    match dsql_project::init_project(dir, Some(hostile.clone())).await {
        Ok(project) => {
            assert_eq!(project.config.database_url, hostile);
            let reloaded = Project::load_from(dir).await.expect("round-trips");
            assert_eq!(reloaded.config.database_url, hostile);
        }
        Err(error) => {
            assert!(
                !dir.join("dsql/dsql.toml").exists(),
                "a rejected URL must not leave a config behind: {error}"
            );
        }
    }
}

#[tokio::test]
async fn schema_directory_round_trips_and_drops_stale_tables() {
    let fixture = fixture_dir("imdb");
    let project = Project::load_from(&fixture)
        .await
        .expect("imdb fixture loads");
    let metadata = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("schema loads");

    let scratch = scratch();
    let dir = scratch.path();
    dsql_project::store_metadata_dir(&metadata, dir)
        .await
        .expect("schema stores");
    let reloaded = dsql_project::load_metadata_dir(dir)
        .await
        .expect("stored schema loads");
    let mut canonical = metadata.clone();
    canonical.canonicalize();
    assert_eq!(reloaded, canonical);

    // A table file for a dropped table disappears on the next store.
    let stale = dir.join("public/dropped_table.yaml");
    write_file(dir, "public/dropped_table.yaml", "stale");
    dsql_project::store_metadata_dir(&metadata, dir)
        .await
        .expect("schema restores");
    assert!(!stale.exists(), "stale table files are removed");
}

#[cfg(unix)]
#[tokio::test]
async fn transactional_schema_publication_preserves_the_live_generation_on_stage_failure() {
    let fixture = fixture_dir("imdb");
    let project = Project::load_from(&fixture)
        .await
        .expect("imdb fixture loads");
    let metadata = dsql_project::load_metadata_dir(&project.schema)
        .await
        .expect("fixture schema loads");
    let scratch = scratch();
    let dsql_root = scratch.path().join("dsql");
    let schema = dsql_root.join("schema");
    dsql_project::store_metadata_dir(&metadata, &schema)
        .await
        .expect("initial generation publishes");

    let leftover = dsql_root.join(".schema.backup-old");
    write_file(
        &leftover,
        "public/poison.yaml",
        "this is not catalog metadata",
    );
    let live = dsql_project::load_metadata_dir(&schema)
        .await
        .expect("leftover sibling is never loaded");

    let original_permissions = std::fs::metadata(&dsql_root)
        .expect("dsql root metadata")
        .permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_mode(0o500);
    std::fs::set_permissions(&dsql_root, read_only).expect("make publication parent read-only");
    let failed = dsql_project::store_metadata_dir(
        &DatabaseMetadata {
            schemas: Vec::new(),
            types: Vec::new(),
        },
        &schema,
    )
    .await;
    std::fs::set_permissions(&dsql_root, original_permissions)
        .expect("restore publication permissions");
    assert!(failed.is_err(), "staging must fail in a read-only parent");

    let after = dsql_project::load_metadata_dir(&schema)
        .await
        .expect("live generation remains readable");
    assert_eq!(after, live, "failed staging cannot mix catalog generations");

    let backup = dsql_root.join(".schema.backup");
    std::fs::rename(&schema, &backup).expect("simulate a crash in the rename window");
    write_file(
        &dsql_root.join(".schema.stage-interrupted"),
        "public/poison.yaml",
        "this is not catalog metadata",
    );
    let recovered = dsql_project::load_metadata_dir(&schema)
        .await
        .expect("the next load restores the last complete generation");
    assert_eq!(recovered, live);
    assert!(schema.exists(), "recovery restores the canonical live path");
    assert!(
        !backup.exists(),
        "the recovered backup no longer competes with the live generation"
    );
}

/// Single-star globs must not cross directory boundaries (the manual
/// reserved-pruning walk keeps glob::glob's literal-separator behavior).
#[tokio::test]
async fn single_star_globs_stay_in_their_directory() {
    let scratch = scratch_project(
        "database_url = \"x\"\n\n[resolution.flat]\ndocuments = [{ resolver = \"dsql\", paths = [\"queries/*.dsql\"] }]\n",
    );
    let dir = scratch.path();
    let doc = "query Q {\n  title(limit 1) {\n    id\n  }\n}\n";
    write_file(dir, "queries/top.dsql", doc);
    write_file(dir, "queries/nested/deep.dsql", doc);

    let project = Project::load_from(dir).await.expect("project loads");
    let documents = dsql_project::load_project_documents(&project)
        .await
        .expect("documents load");
    let paths: Vec<String> = documents
        .iter()
        .map(|document| document.path.display().to_string())
        .collect();
    assert!(
        paths.iter().any(|path| path.ends_with("top.dsql")),
        "the flat file matches, got {paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.ends_with("deep.dsql")),
        "a single star must not cross directories, got {paths:?}"
    );
}
