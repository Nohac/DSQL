//! Catalog lookup behavior independent of the language pipeline.

use std::collections::BTreeSet;

use dsql_core::catalog::{
    Catalog, ColumnMetadata, DataType, DatabaseMetadata, IndexKeyCapability, ObjectType,
    SchemaMetadata, TableMetadata, TableRef, TableResolution, TypeKey, TypeMetadata,
    TypeMetadataFile, table_metadata_from_yaml, table_metadata_to_yaml,
    type_metadata_file_from_yaml, type_metadata_file_to_yaml,
};
use dsql_core::entities::aggregate::AggregateFunction;

fn column(name: &str, provider_type: TypeKey, data_type: DataType) -> ColumnMetadata {
    ColumnMetadata {
        name: name.to_string(),
        description: None,
        database_type: provider_type.name.clone(),
        provider_type,
        data_type,
        not_null: true,
    }
}

fn metadata(columns: Vec<ColumnMetadata>, types: Vec<TypeMetadata>) -> DatabaseMetadata {
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![TableMetadata {
                schema: "public".to_string(),
                name: "records".to_string(),
                object_type: ObjectType::Table,
                description: None,
                columns,
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types,
    }
}

fn provider_type(schema: &str, name: &str) -> TypeMetadata {
    TypeMetadata {
        internal_type: name.to_string(),
        readable_type: name.to_string(),
        schema: schema.to_string(),
        operations: BTreeSet::new(),
    }
}

fn render_builtin_capabilities() -> String {
    DataType::ALL
        .into_iter()
        .map(|data_type| {
            let capabilities = Catalog::builtin_capabilities(data_type);
            let aliases = capabilities.aliases.join(", ");
            let operators = capabilities
                .operators
                .iter()
                .map(|operator| operator.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let literals = capabilities
                .literals
                .kinds
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let defaults = capabilities
                .defaults
                .kinds
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let aggregate = |function| {
                capabilities
                    .aggregates
                    .result(function, data_type)
                    .map_or("-", DataType::as_str)
            };
            format!(
                "{data_type:?}\n  name: {}\n  aliases: [{aliases}]\n  description: {}\n  operators: [{operators}]\n  orderable: {}\n  literals: [{literals}] / {:?} / {}\n  defaults: [{defaults}] / {:?} / {}\n  aggregates: min={} max={} sum={} avg={}",
                capabilities.name,
                capabilities.description,
                capabilities.orderable,
                capabilities.literals.validation,
                capabilities.literals.description,
                capabilities.defaults.validation,
                capabilities.defaults.description,
                aggregate(AggregateFunction::Min),
                aggregate(AggregateFunction::Max),
                aggregate(AggregateFunction::Sum),
                aggregate(AggregateFunction::Avg),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_resolution(catalog: &Catalog, reference: &str) -> String {
    match catalog.resolve_table_ref_for(TableRef::parse(reference)) {
        TableResolution::Found(table) => format!("found {}::{}", table.schema, table.name),
        TableResolution::NotFound { reference } => format!("not found {reference}"),
        TableResolution::Ambiguous {
            reference,
            candidates,
        } => format!(
            "ambiguous {reference}: {}",
            candidates
                .iter()
                .map(|candidate| format!("{}::{}", candidate.schema, candidate.table))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[test]
fn unqualified_tables_resolve_across_visible_schemas() {
    let catalog = Catalog::hardcoded().with_default_schema("other_schema");
    let rendered = [
        "posts",
        "users",
        "public::users",
        "other_schema::users",
        "missing",
    ]
    .map(|reference| format!("{reference}: {}", render_resolution(&catalog, reference)))
    .join("\n");

    insta::assert_snapshot!(rendered);
}

#[test]
fn rich_index_metadata_round_trips_and_drives_catalog_capabilities() {
    let table = table_metadata_from_yaml(
        r#"---
schema: public
name: documents
object_type: table
columns:
  - name: tenant_id
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
    not_null: true
  - name: body
    provider_type:
      schema: pg_catalog
      name: text
    database_type: text
    data_type: text
    not_null: true
  - name: summary
    provider_type:
      schema: pg_catalog
      name: text
    database_type: text
    data_type: text
    not_null: true
  - name: caption
    provider_type:
      schema: pg_catalog
      name: text
    database_type: text
    data_type: text
    not_null: true
constraints: []
foreign_keys: []
indexes:
  - name: documents_tenant_title_idx
    access_method: btree
    keys:
      - column: tenant_id
        operator_class: pg_catalog.uuid_ops
        capabilities: [equality, range, like]
        order:
          direction: asc
          nulls: last
      - column: title
        operator_class: pg_catalog.text_ops
        capabilities: [equality, range]
        order:
          direction: desc
          nulls: first
    included_columns: [summary]
    unique: true
  - name: documents_body_search_idx
    access_method: gin
    keys:
      - column: body
        operator_class: public.gin_trgm_ops
        capabilities: [like]
    unique: false
  - name: documents_caption_lookup_idx
    access_method: btree
    keys:
      - column: caption
        operator_class: pg_catalog.text_ops
        capabilities: [equality, range]
        order:
          direction: asc
          nulls: last
    unique: false
"#,
    )
    .expect("embedded table metadata parses");
    let yaml = table_metadata_to_yaml(&table).expect("table metadata serializes");
    assert!(
        yaml.lines().all(|line| !line.ends_with(' ')),
        "published catalog YAML must pass diff whitespace checks"
    );
    let restored = table_metadata_from_yaml(&yaml).expect("serialized metadata parses");
    let catalog = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![restored],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("embedded catalog builds");
    let table = catalog
        .table("public", "documents")
        .expect("table resolves");
    let columns = ["tenant_id", "title", "body", "summary", "caption"].map(|name| {
        catalog
            .columns_for_table(table.id)
            .find(|column| column.name == name)
            .expect("column resolves")
    });

    assert!(
        catalog.column_is_independently_indexed(columns[0].id),
        "the leading btree key is independently usable"
    );
    // The test-only index capability isolates the catalog type gate: UUID does
    // not support LIKE even when an independently usable key claims it does.
    assert!(!catalog.column_is_searchable(columns[0].id));
    assert!(
        !catalog.column_is_independently_indexed(columns[1].id),
        "a trailing btree key is not independently usable"
    );
    assert!(
        catalog.column_is_independently_indexed(columns[2].id),
        "a gin key is independently usable"
    );
    assert!(
        !catalog.column_participates_in_index(columns[3].id),
        "included columns are not index keys"
    );
    assert!(
        !catalog.column_is_searchable(columns[1].id),
        "a trailing btree key is not independently searchable"
    );
    assert!(catalog.column_is_searchable(columns[2].id));
    assert!(
        catalog.column_is_independently_indexed(columns[4].id),
        "the leading text btree key is independently usable"
    );
    assert!(
        !catalog.column_is_searchable(columns[4].id),
        "a leading text key without native LIKE support is not searchable"
    );
    assert!(
        table.indexes[1].keys[0]
            .capabilities
            .contains(&IndexKeyCapability::Like)
    );
    assert!(catalog.column_set_is_unique(table.id, &[columns[0].id, columns[1].id]));
    assert!(
        !catalog.column_set_is_unique(table.id, &[columns[0].id]),
        "a strict prefix does not prove composite uniqueness"
    );
    assert!(
        !catalog.column_set_is_unique(table.id, &[columns[3].id]),
        "included columns do not prove uniqueness"
    );

    insta::assert_snapshot!(yaml);
}

#[test]
fn duplicate_provider_type_identities_are_rejected() {
    let data_type = || TypeMetadata {
        internal_type: "person".to_string(),
        readable_type: "person".to_string(),
        schema: "app".to_string(),
        operations: BTreeSet::new(),
    };
    let error = DatabaseMetadata {
        schemas: Vec::new(),
        types: vec![data_type(), data_type()],
    }
    .to_catalog()
    .expect_err("duplicate qualified provider type fails");

    insta::assert_snapshot!(error);
}

#[test]
fn catalog_columns_share_only_qualified_type_identities() {
    let alpha_text = TypeKey::new("alpha", "text");
    let beta_text = TypeKey::new("beta", "text");
    let catalog = metadata(
        vec![
            column("first", alpha_text.clone(), DataType::Text),
            column("second", alpha_text, DataType::Text),
            column("third", beta_text, DataType::Text),
        ],
        vec![
            provider_type("alpha", "text"),
            provider_type("beta", "text"),
        ],
    )
    .to_catalog()
    .expect("catalog builds");

    assert_eq!(catalog.columns[0].type_id, catalog.columns[1].type_id);
    assert_ne!(catalog.columns[0].type_id, catalog.columns[2].type_id);
    assert_eq!(
        catalog.data_type_for_column(catalog.columns[0].id),
        DataType::Text
    );
}

#[test]
fn catalog_type_arena_rejects_invalid_metadata() {
    let missing = metadata(
        vec![column(
            "value",
            TypeKey::new("pg_catalog", "uuid"),
            DataType::Uuid,
        )],
        vec![provider_type("pg_catalog", "text")],
    )
    .to_catalog()
    .expect_err("strict metadata rejects a missing type");
    let mismatch = metadata(
        vec![column(
            "value",
            TypeKey::new("pg_catalog", "text"),
            DataType::Uuid,
        )],
        vec![provider_type("pg_catalog", "text")],
    )
    .to_catalog()
    .expect_err("strict metadata rejects a logical mismatch");
    let fixture_conflict = metadata(
        vec![
            column("first", TypeKey::new("fixture", "value"), DataType::Text),
            column("second", TypeKey::new("fixture", "value"), DataType::Uuid),
        ],
        Vec::new(),
    )
    .to_catalog()
    .expect_err("fixture synthesis rejects conflicting logical types");

    insta::assert_snapshot!(format!("{missing}\n{mismatch}\n{fixture_conflict}"));
}

#[test]
fn catalog_type_declaration_order_and_unused_rows_are_fingerprint_neutral() {
    let columns = vec![
        column("label", TypeKey::new("alpha", "text"), DataType::Text),
        column("identifier", TypeKey::new("beta", "uuid"), DataType::Uuid),
    ];
    let first = provider_type("alpha", "text");
    let second = provider_type("beta", "uuid");
    let baseline_catalog = metadata(columns.clone(), vec![first.clone(), second.clone()])
        .to_catalog()
        .expect("baseline catalog builds");
    let baseline = baseline_catalog.semantic_fingerprint();
    let reordered = metadata(columns.clone(), vec![second.clone(), first.clone()])
        .to_catalog()
        .expect("reordered catalog builds")
        .semantic_fingerprint();
    let mut with_unused_catalog = metadata(
        columns,
        vec![first, second, provider_type("unused", "bool")],
    )
    .to_catalog()
    .expect("catalog with unused type builds");
    let with_unused = with_unused_catalog.semantic_fingerprint();

    assert_eq!(baseline, reordered);
    assert_eq!(baseline, with_unused);

    let mut changed_capabilities = baseline_catalog;
    changed_capabilities.types[0].capabilities.orderable = false;
    assert_ne!(baseline, changed_capabilities.semantic_fingerprint());

    let unused_type = with_unused_catalog
        .types
        .last_mut()
        .expect("unused provider type exists");
    unused_type.capabilities.orderable = false;
    assert_eq!(baseline, with_unused_catalog.semantic_fingerprint());
}

#[test]
fn builtin_type_capabilities_are_declared_in_one_matrix() {
    insta::assert_snapshot!(render_builtin_capabilities());
}

#[test]
fn qualified_provider_type_metadata_round_trips() {
    let types = TypeMetadataFile {
        types: vec![
            TypeMetadata {
                internal_type: "person".to_string(),
                readable_type: "alpha.person".to_string(),
                schema: "alpha".to_string(),
                operations: BTreeSet::new(),
            },
            TypeMetadata {
                internal_type: "person".to_string(),
                readable_type: "beta.person".to_string(),
                schema: "beta".to_string(),
                operations: BTreeSet::from(["=".to_string()]),
            },
        ],
    };
    let yaml = type_metadata_file_to_yaml(&types).expect("type metadata serializes");
    assert_eq!(
        type_metadata_file_from_yaml(&yaml).expect("type metadata round-trips"),
        types
    );

    insta::assert_snapshot!(yaml);
}
