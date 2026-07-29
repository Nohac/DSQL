//! Catalog lookup behavior independent of the language pipeline.

use std::collections::BTreeSet;

use dsql_core::catalog::{
    Catalog, CatalogTypeShape, CatalogValueShape, ColumnMetadata, DataType, DatabaseMetadata,
    EnumTypeMetadata, EnumVariantMetadata, IndexKeyCapability, ObjectType, ProviderTypeFacts,
    SchemaMetadata, TableMetadata, TableRef, TableResolution, TypeKey, TypeMetadata,
    TypeMetadataFile, TypeStructureKind, TypeStructureMetadata, WireEncoding,
    table_metadata_from_yaml, table_metadata_to_yaml, type_metadata_file_from_yaml,
    type_metadata_file_to_yaml,
};
use dsql_core::entities::aggregate::AggregateFunction;
use dsql_core::entities::expression::ComparisonOp;

use super::structured_type_catalog;

fn column(name: &str, provider_type: TypeKey, data_type: DataType) -> ColumnMetadata {
    ColumnMetadata {
        name: name.to_string(),
        description: None,
        database_type: provider_type.name.clone(),
        provider_type,
        formatted_type: None,
        type_modifier: None,
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
        structure: TypeStructureMetadata::scalar(),
        provider: None,
        operations: BTreeSet::new(),
    }
}

fn provider_type_with_facts(
    schema: &str,
    name: &str,
    operations: &[&str],
    orderable: bool,
) -> TypeMetadata {
    TypeMetadata {
        internal_type: name.to_string(),
        readable_type: format!("{schema}.{name}"),
        schema: schema.to_string(),
        structure: TypeStructureMetadata::scalar(),
        provider: Some(ProviderTypeFacts {
            kind: "b".to_string(),
            category: "U".to_string(),
            effective_kind: None,
            effective_category: None,
            orderable,
        }),
        operations: operations
            .iter()
            .map(|operation| (*operation).to_string())
            .collect(),
    }
}

fn native_enum_type(
    schema: &str,
    name: &str,
    description: Option<&str>,
    variants: &[&str],
) -> TypeMetadata {
    TypeMetadata {
        internal_type: name.to_string(),
        readable_type: format!("{schema}.{name}"),
        schema: schema.to_string(),
        structure: TypeStructureMetadata::enumeration(EnumTypeMetadata {
            description: description.map(str::to_string),
            variants: variants
                .iter()
                .map(|variant| EnumVariantMetadata {
                    variant: (*variant).to_string(),
                    database_value: (*variant).to_string(),
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
                "{data_type:?}\n  name: {}\n  aliases: [{aliases}]\n  description: {}\n  wire: {:?}\n  operators: [{operators}]\n  orderable: {}\n  literals: [{literals}] / {:?} / {}\n  defaults: [{defaults}] / {:?} / {}\n  aggregates: min={} max={} sum={} avg={}",
                capabilities.name,
                capabilities.description,
                capabilities.wire,
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
fn bigint_has_one_stable_serialized_name() {
    let yaml = facet_yaml::to_string(&DataType::BigInt).expect("bigint serializes to YAML");
    let json = facet_json::to_string(&DataType::BigInt).expect("bigint serializes to JSON");

    assert_eq!(yaml.trim(), "---\nbigint");
    assert_eq!(json, "\"bigint\"");
    assert_eq!(
        facet_yaml::from_str::<DataType>(&yaml).expect("bigint parses from YAML"),
        DataType::BigInt
    );
    assert_eq!(
        facet_json::from_str::<DataType>(&json).expect("bigint parses from JSON"),
        DataType::BigInt
    );
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
      name: varchar
    formatted_type: character varying(20)
    type_modifier: 24
    database_type: varchar
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
        structure: TypeStructureMetadata::scalar(),
        provider: None,
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
fn catalog_type_lookup_survives_a_skipped_derived_index() {
    let key = TypeKey::new("pg_catalog", "uuid");
    // The hand-authored fixture intentionally leaves the derived index empty.
    let catalog = Catalog::hardcoded();

    assert_eq!(
        catalog.type_by_key(&key).map(|data_type| &data_type.key),
        Some(&key)
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
    let stale_bigint = metadata(
        vec![column(
            "value",
            TypeKey::new("pg_catalog", "int8"),
            DataType::Int,
        )],
        vec![provider_type("pg_catalog", "int8")],
    )
    .to_catalog()
    .expect_err("an int8 cache with the wrong logical contract is rejected");
    let fixture_conflict = metadata(
        vec![
            column("first", TypeKey::new("fixture", "value"), DataType::Text),
            column("second", TypeKey::new("fixture", "value"), DataType::Uuid),
        ],
        Vec::new(),
    )
    .to_catalog()
    .expect_err("fixture synthesis rejects conflicting logical types");

    insta::assert_snapshot!(format!(
        "{missing}\n{mismatch}\n{stale_bigint}\n{fixture_conflict}"
    ));
}

#[test]
fn catalog_type_arena_rejects_invalid_structure_graphs() {
    let mut scalar_with_base = provider_type("public", "invalid_scalar");
    scalar_with_base.structure.related_type = Some(TypeKey::new("pg_catalog", "text"));

    let mut domain_without_base = provider_type("public", "invalid_domain");
    domain_without_base.structure.kind = TypeStructureKind::Domain;

    let missing_base = TypeMetadata {
        internal_type: "missing_base".to_string(),
        readable_type: "public.missing_base".to_string(),
        schema: "public".to_string(),
        structure: TypeStructureMetadata::domain(TypeKey::new("public", "absent")),
        provider: None,
        operations: BTreeSet::new(),
    };

    let cycle_left = TypeMetadata {
        internal_type: "cycle_left".to_string(),
        readable_type: "public.cycle_left[]".to_string(),
        schema: "public".to_string(),
        structure: TypeStructureMetadata::array(TypeKey::new("public", "cycle_right")),
        provider: None,
        operations: BTreeSet::new(),
    };
    let cycle_right = TypeMetadata {
        internal_type: "cycle_right".to_string(),
        readable_type: "public.cycle_right[]".to_string(),
        schema: "public".to_string(),
        structure: TypeStructureMetadata::array(TypeKey::new("public", "cycle_left")),
        provider: None,
        operations: BTreeSet::new(),
    };

    let cases = [
        ("scalar edge", vec![scalar_with_base]),
        ("missing domain edge", vec![domain_without_base]),
        ("missing base", vec![missing_base]),
        ("cycle", vec![cycle_left, cycle_right]),
    ];
    let rendered = cases
        .into_iter()
        .map(|(name, types)| {
            let error = metadata(Vec::new(), types)
                .to_catalog()
                .expect_err("invalid structure graph must fail");
            format!("{name}: {error}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    insta::assert_snapshot!(rendered);
}

#[test]
fn native_enum_catalog_facts_are_nominal_and_structural() {
    let status_key = TypeKey::new("public", "status");
    let status_domain_key = TypeKey::new("public", "status_domain");
    let status_array_key = TypeKey::new("public", "_status");
    let mut catalog_metadata = metadata(
        vec![
            column("status", status_key.clone(), DataType::Unknown),
            column(
                "domain_status",
                status_domain_key.clone(),
                DataType::Unknown,
            ),
            column("statuses", status_array_key.clone(), DataType::Unknown),
        ],
        vec![
            native_enum_type(
                "public",
                "status",
                Some("Lifecycle status."),
                &["pending", "active", "archived"],
            ),
            native_enum_type("audit", "status", None, &["pending", "active", "archived"]),
            TypeMetadata {
                internal_type: "status_domain".to_string(),
                readable_type: "public.status_domain".to_string(),
                schema: "public".to_string(),
                structure: TypeStructureMetadata::domain(status_key.clone()),
                provider: Some(ProviderTypeFacts {
                    kind: "d".to_string(),
                    category: "E".to_string(),
                    effective_kind: Some("e".to_string()),
                    effective_category: Some("E".to_string()),
                    orderable: true,
                }),
                operations: ["=", "<>"].into_iter().map(str::to_string).collect(),
            },
            TypeMetadata {
                internal_type: "_status".to_string(),
                readable_type: "public.status[]".to_string(),
                schema: "public".to_string(),
                structure: TypeStructureMetadata::array(status_key.clone()),
                provider: Some(ProviderTypeFacts {
                    kind: "b".to_string(),
                    category: "A".to_string(),
                    effective_kind: Some("b".to_string()),
                    effective_category: Some("A".to_string()),
                    orderable: true,
                }),
                operations: ["=", "<>"].into_iter().map(str::to_string).collect(),
            },
        ],
    );
    let catalog = catalog_metadata
        .to_catalog()
        .expect("native enum structures build");

    let status = catalog
        .type_by_key(&status_key)
        .expect("native enum type exists");
    let enumeration = status
        .enumeration
        .as_ref()
        .expect("native enum payload exists");
    assert!(matches!(status.shape, CatalogTypeShape::Enum));
    assert_eq!(status.capabilities.wire, WireEncoding::TextCast);
    assert!(status.capabilities.supports(ComparisonOp::Eq));
    assert!(status.capabilities.orderable);
    assert_eq!(
        enumeration.description.as_deref(),
        Some("Lifecycle status.")
    );
    assert_eq!(
        enumeration
            .variants
            .iter()
            .map(|variant| variant.variant.as_str())
            .collect::<Vec<_>>(),
        ["pending", "active", "archived"],
        "provider enum order is semantic rather than lexical"
    );

    let status_domain = catalog
        .type_by_key(&status_domain_key)
        .expect("domain type exists");
    let (domain_enum_type, _) = catalog
        .enum_type_for_type(status_domain.id)
        .expect("domains resolve to their enum base");
    assert_eq!(domain_enum_type.key, status_key);

    let status_array = catalog
        .type_by_key(&status_array_key)
        .expect("array type exists");
    assert!(
        catalog.enum_type_for_type(status_array.id).is_none(),
        "arrays remain collection shapes rather than resolving as enums"
    );
    assert!(
        matches!(status_array.shape, CatalogTypeShape::Array { .. }),
        "status array keeps its array shape"
    );
    if let CatalogTypeShape::Array { element } = status_array.shape {
        let (array_enum_type, _) = catalog
            .enum_type_for_type(element)
            .expect("an array element resolves to its enum type");
        assert_eq!(array_enum_type.key, status_key);
    }

    let value_shape = |column_name: &str| {
        let column = catalog
            .columns
            .iter()
            .find(|column| column.name == column_name)
            .expect("enum test column exists");
        catalog
            .value_shape_for_column(column.id)
            .expect("enum test column has a public value shape")
    };
    assert!(matches!(
        value_shape("status"),
        CatalogValueShape::Scalar { leaf } if leaf.key == status_key
    ));
    assert!(matches!(
        value_shape("domain_status"),
        CatalogValueShape::Scalar { leaf } if leaf.key == status_domain_key
    ));
    assert!(matches!(
        value_shape("statuses"),
        CatalogValueShape::DatabaseArray { element } if element.key == status_key
    ));

    let audit_status = catalog
        .type_by_key(&TypeKey::new("audit", "status"))
        .expect("same-named enum in another schema exists");
    assert_ne!(status.id, audit_status.id);
    assert_eq!(
        status
            .enumeration
            .as_ref()
            .map(|enumeration| &enumeration.variants),
        audit_status
            .enumeration
            .as_ref()
            .map(|enumeration| &enumeration.variants),
        "identical variant sets do not collapse nominal enum identities"
    );

    let fingerprint = catalog.semantic_fingerprint();
    let public_status = catalog_metadata
        .types
        .iter_mut()
        .find(|data_type| data_type.key() == status_key)
        .expect("public enum metadata");
    public_status
        .structure
        .enumeration
        .as_mut()
        .expect("enum metadata")
        .variants
        .swap(0, 1);
    assert_ne!(
        fingerprint,
        catalog_metadata
            .to_catalog()
            .expect("reordered enum builds")
            .semantic_fingerprint(),
        "enum ordering participates in the catalog fingerprint"
    );
}

#[test]
fn native_enum_catalog_facts_reject_invalid_snapshots() {
    let mut stale = native_enum_type("public", "stale", None, &["one"]);
    stale.structure = TypeStructureMetadata::scalar();

    let enumeration = native_enum_type("public", "payload", None, &["one"])
        .structure
        .enumeration
        .expect("native enum helper produces metadata");
    let mut scalar_payload = provider_type_with_facts("public", "scalar_payload", &[], false);
    scalar_payload.structure.enumeration = Some(enumeration.clone());
    let mut domain_payload = provider_type_with_facts("public", "domain_payload", &[], false);
    domain_payload.structure.kind = TypeStructureKind::Domain;
    domain_payload.structure.enumeration = Some(enumeration.clone());
    let mut array_payload = provider_type_with_facts("public", "array_payload", &[], false);
    array_payload.structure.kind = TypeStructureKind::Array;
    array_payload.structure.enumeration = Some(enumeration);

    let empty = native_enum_type("public", "empty", None, &[]);

    let mut missing = native_enum_type("public", "missing", None, &["one"]);
    missing.structure.enumeration = None;

    let mut related = native_enum_type("public", "related", None, &["one"]);
    related.structure.related_type = Some(TypeKey::new("pg_catalog", "text"));

    let mut configured = native_enum_type("public", "configured", None, &["one"]);
    configured.provider = None;

    let mut duplicate = native_enum_type("public", "duplicate", None, &["one", "one"]);
    duplicate
        .structure
        .enumeration
        .as_mut()
        .expect("enum metadata")
        .variants[1]
        .database_value = "other".to_string();

    let mut duplicate_database =
        native_enum_type("public", "duplicate_database", None, &["one", "two"]);
    duplicate_database
        .structure
        .enumeration
        .as_mut()
        .expect("enum metadata")
        .variants[1]
        .database_value = "one".to_string();

    let mut mismatched = native_enum_type("public", "mismatched", None, &["one"]);
    mismatched
        .structure
        .enumeration
        .as_mut()
        .expect("enum metadata")
        .variants[0]
        .database_value = "1".to_string();

    let cases = [
        ("stale scalar", stale),
        ("scalar payload", scalar_payload),
        ("domain payload", domain_payload),
        ("array payload", array_payload),
        ("empty", empty),
        ("missing", missing),
        ("related", related),
        ("configured", configured),
        ("duplicate", duplicate),
        ("duplicate database", duplicate_database),
        ("mismatched", mismatched),
    ];
    let rendered = cases
        .into_iter()
        .map(|(name, data_type)| {
            let error = metadata(Vec::new(), vec![data_type])
                .to_catalog()
                .expect_err("invalid native enum metadata must fail");
            format!("{name}: {error}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    insta::assert_snapshot!(rendered);
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

    let mut changed_capabilities = baseline_catalog.clone();
    changed_capabilities.types[0].capabilities.orderable = false;
    assert_ne!(baseline, changed_capabilities.semantic_fingerprint());

    let unused_type = with_unused_catalog
        .types
        .last_mut()
        .expect("unused provider type exists");
    unused_type.capabilities.orderable = false;
    assert_eq!(baseline, with_unused_catalog.semantic_fingerprint());

    let mut changed_provider = baseline_catalog.clone();
    changed_provider.types[0].provider = Some(ProviderTypeFacts {
        kind: "b".to_string(),
        category: "S".to_string(),
        effective_kind: None,
        effective_category: None,
        orderable: true,
    });
    assert_ne!(baseline, changed_provider.semantic_fingerprint());

    let mut changed_readable_type = baseline_catalog.clone();
    changed_readable_type.types[0].readable_type = "display text".to_string();
    assert_ne!(baseline, changed_readable_type.semantic_fingerprint());

    let mut changed_formatted_type = baseline_catalog.clone();
    changed_formatted_type.columns[0].formatted_type = "character varying(20)".to_string();
    assert_ne!(baseline, changed_formatted_type.semantic_fingerprint());

    let mut changed_type_modifier = baseline_catalog;
    changed_type_modifier.columns[0].type_modifier = Some(24);
    assert_ne!(baseline, changed_type_modifier.semantic_fingerprint());

    with_unused_catalog
        .types
        .last_mut()
        .expect("unused provider type exists")
        .provider = Some(ProviderTypeFacts {
        kind: "b".to_string(),
        category: "B".to_string(),
        effective_kind: None,
        effective_category: None,
        orderable: true,
    });
    assert_eq!(baseline, with_unused_catalog.semantic_fingerprint());
}

#[test]
fn catalog_fingerprint_closes_over_reachable_type_structure() {
    let text = provider_type_with_facts("pg_catalog", "text", &["=", "<>"], true);
    let mut text_array = provider_type_with_facts("pg_catalog", "_text", &["=", "<>"], true);
    text_array.structure = TypeStructureMetadata::array(TypeKey::new("pg_catalog", "text"));
    let unused = provider_type_with_facts("pg_catalog", "uuid", &["=", "<>"], true);
    let columns = vec![column(
        "values",
        TypeKey::new("pg_catalog", "_text"),
        DataType::Unknown,
    )];
    let baseline_catalog = metadata(
        columns.clone(),
        vec![text.clone(), text_array.clone(), unused.clone()],
    )
    .to_catalog()
    .expect("array catalog builds");
    let baseline = baseline_catalog.semantic_fingerprint();
    let reordered = metadata(columns, vec![unused, text_array, text])
        .to_catalog()
        .expect("reordered array catalog builds")
        .semantic_fingerprint();
    assert_eq!(baseline, reordered);

    let mut changed_element = baseline_catalog.clone();
    let element = changed_element
        .types
        .iter_mut()
        .find(|data_type| data_type.key == TypeKey::new("pg_catalog", "text"))
        .expect("reachable array element");
    element.capabilities.orderable = false;
    assert_ne!(baseline, changed_element.semantic_fingerprint());

    let mut changed_unused = baseline_catalog;
    let unused = changed_unused
        .types
        .iter_mut()
        .find(|data_type| data_type.key == TypeKey::new("pg_catalog", "uuid"))
        .expect("unused provider type");
    unused.capabilities.orderable = false;
    assert_eq!(baseline, changed_unused.semantic_fingerprint());
}

#[test]
fn recursive_type_structures_resolve_to_terminal_public_shapes() {
    let catalog = structured_type_catalog();
    let shape = |column_name: &str| {
        let column = catalog
            .columns
            .iter()
            .find(|column| column.name == column_name)
            .expect("structured column");
        (
            catalog.data_type_for_column(column.id),
            catalog
                .value_shape_for_column(column.id)
                .expect("public value shape"),
        )
    };

    let (nested_domain_type, nested_domain_shape) = shape("nested_label");
    assert_eq!(nested_domain_type, DataType::Text);
    assert!(matches!(
        nested_domain_shape,
        CatalogValueShape::Scalar { leaf }
            if leaf.key == TypeKey::new("public", "nested_label_domain")
    ));

    let (domain_array_type, domain_array_shape) = shape("domain_labels");
    assert_eq!(domain_array_type, DataType::Unknown);
    assert!(matches!(
        domain_array_shape,
        CatalogValueShape::DatabaseArray { element }
            if element.key == TypeKey::new("pg_catalog", "text")
    ));

    let (array_domain_type, array_domain_shape) = shape("labeled_values");
    assert_eq!(array_domain_type, DataType::Unknown);
    assert!(matches!(
        array_domain_shape,
        CatalogValueShape::DatabaseArray { element }
            if element.key == TypeKey::new("public", "label_domain")
    ));

    let (nested_array_type, nested_array_shape) = shape("nested_domain_labels");
    assert_eq!(nested_array_type, DataType::Unknown);
    assert!(matches!(
        nested_array_shape,
        CatalogValueShape::DatabaseArray { element }
            if element.key == TypeKey::new("pg_catalog", "text")
    ));
}

#[test]
fn builtin_type_capabilities_are_declared_in_one_matrix() {
    insta::assert_snapshot!(render_builtin_capabilities());
}

#[test]
fn provider_comparison_capabilities_override_compiler_capabilities() {
    let providerless_json = metadata(
        vec![column(
            "payload",
            TypeKey::new("pg_catalog", "json"),
            DataType::Json,
        )],
        vec![provider_type("pg_catalog", "json")],
    )
    .to_catalog()
    .expect("provider-less compiler metadata builds");
    let provider_json = metadata(
        vec![column(
            "payload",
            TypeKey::new("pg_catalog", "json"),
            DataType::Json,
        )],
        vec![provider_type_with_facts("pg_catalog", "json", &[], false)],
    )
    .to_catalog()
    .expect("fresh provider metadata builds");
    let provider_citext = metadata(
        vec![column(
            "label",
            TypeKey::new("public", "citext"),
            DataType::Unknown,
        )],
        vec![provider_type_with_facts(
            "public",
            "citext",
            &["=", "<>", "~~"],
            true,
        )],
    )
    .to_catalog()
    .expect("extension provider metadata builds");
    let provider_domain = metadata(
        vec![column(
            "label",
            TypeKey::new("public", "label_domain"),
            DataType::Text,
        )],
        vec![
            provider_type_with_facts("pg_catalog", "text", &["=", "<>", "~~"], true),
            TypeMetadata {
                internal_type: "label_domain".to_string(),
                readable_type: "public.label_domain".to_string(),
                schema: "public".to_string(),
                structure: TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "text")),
                provider: Some(ProviderTypeFacts {
                    kind: "d".to_string(),
                    category: "U".to_string(),
                    effective_kind: Some("b".to_string()),
                    effective_category: Some("S".to_string()),
                    orderable: true,
                }),
                operations: ["=", "<>", ">"].into_iter().map(str::to_string).collect(),
            },
        ],
    )
    .to_catalog()
    .expect("domain provider metadata builds");
    let provider_array = metadata(
        vec![column(
            "labels",
            TypeKey::new("pg_catalog", "_text"),
            DataType::Unknown,
        )],
        vec![
            provider_type_with_facts("pg_catalog", "text", &["=", "<>", "~~"], true),
            TypeMetadata {
                internal_type: "_text".to_string(),
                readable_type: "text[]".to_string(),
                schema: "pg_catalog".to_string(),
                structure: TypeStructureMetadata::array(TypeKey::new("pg_catalog", "text")),
                provider: Some(ProviderTypeFacts {
                    kind: "b".to_string(),
                    category: "A".to_string(),
                    effective_kind: None,
                    effective_category: None,
                    orderable: true,
                }),
                operations: ["=", "<>"].into_iter().map(str::to_string).collect(),
            },
        ],
    )
    .to_catalog()
    .expect("array provider metadata builds");

    let render = |catalog: &Catalog| {
        let data_type = catalog
            .type_for_column(catalog.columns[0].id)
            .expect("column type exists");
        format!(
            "{} ({:?}/{:?}) wire={:?} operators=[{}] orderable={}",
            data_type.readable_type,
            data_type.provider,
            data_type.data_type,
            data_type.capabilities.wire,
            data_type
                .capabilities
                .operators
                .iter()
                .map(|operator| operator.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            data_type.capabilities.orderable,
        )
    };
    insta::assert_snapshot!(format!(
        "provider-less json: {}\nprovider json: {}\nprovider citext: {}\nprovider domain: {}\nprovider array: {}",
        render(&providerless_json),
        render(&provider_json),
        render(&provider_citext),
        render(&provider_domain),
        render(&provider_array),
    ));

    let domain = provider_domain
        .type_for_column(provider_domain.columns[0].id)
        .expect("domain column type");
    assert_eq!(domain.data_type, DataType::Text);
    assert_eq!(domain.capabilities.wire, WireEncoding::Text);
    assert!(
        !domain.capabilities.supports(ComparisonOp::Gt),
        "a domain cannot widen its base type's comparison surface"
    );
    assert!(matches!(domain.shape, CatalogTypeShape::Domain { .. }));

    let array = provider_array
        .type_for_column(provider_array.columns[0].id)
        .expect("array column type");
    assert_eq!(array.data_type, DataType::Unknown);
    assert_eq!(array.capabilities.wire, WireEncoding::Unsupported);
    assert!(matches!(array.shape, CatalogTypeShape::Array { .. }));
    if let CatalogTypeShape::Array { element } = array.shape {
        assert_eq!(provider_array.types[element.0].data_type, DataType::Text);
    }
}

#[test]
fn qualified_provider_type_metadata_round_trips() {
    let types = TypeMetadataFile {
        types: vec![
            TypeMetadata {
                internal_type: "person".to_string(),
                readable_type: "alpha.person".to_string(),
                schema: "alpha".to_string(),
                structure: TypeStructureMetadata::scalar(),
                provider: None,
                operations: BTreeSet::new(),
            },
            native_enum_type(
                "beta",
                "person",
                Some("People participating in review."),
                &["reviewer", "owner"],
            ),
        ],
    };
    let yaml = type_metadata_file_to_yaml(&types).expect("type metadata serializes");
    assert_eq!(
        type_metadata_file_from_yaml(&yaml).expect("type metadata round-trips"),
        types
    );

    insta::assert_snapshot!(yaml);
}
