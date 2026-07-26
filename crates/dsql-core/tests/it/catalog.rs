//! Catalog lookup behavior independent of the language pipeline.

use dsql_core::catalog::{
    Catalog, DatabaseMetadata, IndexKeyCapability, SchemaMetadata, TableRef, TableResolution,
    table_metadata_from_yaml, table_metadata_to_yaml,
};

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
    database_type: uuid
    data_type: uuid
    not_null: true
  - name: title
    database_type: text
    data_type: text
    not_null: true
  - name: body
    database_type: text
    data_type: text
    not_null: true
  - name: summary
    database_type: text
    data_type: text
    not_null: true
  - name: caption
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
        capabilities: [equality, range]
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
