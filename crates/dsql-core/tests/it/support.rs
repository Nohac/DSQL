//! Shared helpers for the integration harness: fixture loading and
//! snapshot-stable renderers.

use std::collections::BTreeSet;
use std::fmt::Write;

use bowl::{Bowl, Entity, Mut, Query};
use codespan_reporting::diagnostic::Severity;
use dsql_core::catalog::{
    Catalog, ColumnMetadata, DataType, DatabaseMetadata, EnumTypeMetadata, EnumVariantMetadata,
    ObjectType, ProviderTypeFacts, SchemaMetadata, TableMetadata, TypeKey, TypeMetadata,
    TypeStructureMetadata, table_metadata_from_yaml,
};
use dsql_core::grammar::parser::Diagnostic;
use dsql_core::source::SourceText;

const QUERY_FIXTURES: &[(&str, &str)] = &[
    (
        "invalid/imdb-duplicate-relation-path.dsql",
        include_str!("queries/invalid/imdb-duplicate-relation-path.dsql"),
    ),
    (
        "invalid/imdb-scalar-clause-list.dsql",
        include_str!("queries/invalid/imdb-scalar-clause-list.dsql"),
    ),
    (
        "invalid/imdb-unknown-column.dsql",
        include_str!("queries/invalid/imdb-unknown-column.dsql"),
    ),
    (
        "valid/imdb-fragment-spread.dsql",
        include_str!("queries/valid/imdb-fragment-spread.dsql"),
    ),
    (
        "valid/imdb-movie-info-basic.dsql",
        include_str!("queries/valid/imdb-movie-info-basic.dsql"),
    ),
    (
        "valid/imdb-relation-path-selector.dsql",
        include_str!("queries/valid/imdb-relation-path-selector.dsql"),
    ),
    (
        "valid/imdb-rhs-relation-path.dsql",
        include_str!("queries/valid/imdb-rhs-relation-path.dsql"),
    ),
    (
        "valid/imdb-rhs-same-table.dsql",
        include_str!("queries/valid/imdb-rhs-same-table.dsql"),
    ),
    (
        "valid/imdb-scoped-relation-predicate.dsql",
        include_str!("queries/valid/imdb-scoped-relation-predicate.dsql"),
    ),
    (
        "valid/imdb-title-basic.dsql",
        include_str!("queries/valid/imdb-title-basic.dsql"),
    ),
];

const IMDB_TABLES: &[&str] = &[
    include_str!("schema/imdb/public/aka_name.yaml"),
    include_str!("schema/imdb/public/aka_title.yaml"),
    include_str!("schema/imdb/public/cast_info.yaml"),
    include_str!("schema/imdb/public/char_name.yaml"),
    include_str!("schema/imdb/public/comp_cast_type.yaml"),
    include_str!("schema/imdb/public/company_name.yaml"),
    include_str!("schema/imdb/public/company_type.yaml"),
    include_str!("schema/imdb/public/complete_cast.yaml"),
    include_str!("schema/imdb/public/info_type.yaml"),
    include_str!("schema/imdb/public/keyword.yaml"),
    include_str!("schema/imdb/public/kind_type.yaml"),
    include_str!("schema/imdb/public/link_type.yaml"),
    include_str!("schema/imdb/public/movie_companies.yaml"),
    include_str!("schema/imdb/public/movie_info.yaml"),
    include_str!("schema/imdb/public/movie_info_idx.yaml"),
    include_str!("schema/imdb/public/movie_keyword.yaml"),
    include_str!("schema/imdb/public/movie_link.yaml"),
    include_str!("schema/imdb/public/name.yaml"),
    include_str!("schema/imdb/public/person_info.yaml"),
    include_str!("schema/imdb/public/role_type.yaml"),
    include_str!("schema/imdb/public/title.yaml"),
];

/// Returns an embedded query fixture by its repository-relative test path.
pub fn fixture(relative_path: &str) -> String {
    QUERY_FIXTURES
        .iter()
        .find_map(|(path, text)| (*path == relative_path).then_some(*text))
        .unwrap_or_else(|| panic!("unknown query fixture {relative_path}"))
        .to_string()
}

/// Replaces one source entity's complete text through the same external
/// mutation path editor integrations use.
pub async fn set_source_text(bowl: &Bowl, file: Entity, text: impl Into<String>) {
    let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
    let source = sources
        .collect()
        .into_iter()
        .find_map(|(entity, source)| (entity == file).then_some(source))
        .expect("source entity must exist");
    let text = text.into();
    source
        .with_latest(move |source| source.set_text(&text))
        .await;
}

/// Replaces every occurrence of `from` in one resident source entity.
pub async fn replace_source_text(
    bowl: &Bowl,
    file: Entity,
    from: impl Into<String>,
    to: impl Into<String>,
) {
    let sources = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
    let source = sources
        .collect()
        .into_iter()
        .find_map(|(entity, source)| (entity == file).then_some(source))
        .expect("source entity must exist");
    let from = from.into();
    let to = to.into();
    source
        .with_latest(move |source| {
            let text = source
                .to_text()
                .expect("edited source text must be resident")
                .replace(&from, &to);
            source.set_text(&text);
        })
        .await;
}

/// Renders every diagnostic fact in a settled bowl, sorted for stability.
pub async fn render_diagnostic_facts(bowl: &Bowl) -> String {
    let rows = bowl
        .scoop::<Query<(
            Entity,
            &dsql_core::facts::Severity,
            &dsql_core::facts::Span,
            &dsql_core::facts::Diagnostic,
        )>>()
        .await;
    let mut lines: Vec<String> = rows
        .collect()
        .into_iter()
        .map(|(_, severity, span, diagnostic)| {
            format!(
                "{severity:?}[{}..{}]: {}",
                span.start, span.end, diagnostic.0
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

/// Renders parse diagnostics into a compact, snapshot-stable form.
pub fn render_diagnostics(source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut rendered = String::new();
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Help => "help",
            Severity::Bug => "bug",
        };
        let span = diagnostic
            .labels
            .first()
            .map(|label| label.range.clone())
            .unwrap_or(0..0);
        let excerpt = source.get(span.clone()).unwrap_or("<out of range>");
        writeln!(
            rendered,
            "{severity}[{}..{}]: {} ({excerpt:?})",
            span.start, span.end, diagnostic.message
        )
        .expect("writing to a String cannot fail");
    }
    rendered
}

/// Builds the imdb catalog from compile-time embedded schema fixtures.
pub fn imdb_catalog() -> Catalog {
    let mut tables = IMDB_TABLES
        .iter()
        .map(|raw| table_metadata_from_yaml(raw).expect("embedded imdb table must parse"))
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
    .expect("imdb fixture catalog must build")
    .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

/// A compact catalog that exercises exact and floating-point PostgreSQL
/// number types through the full language pipeline.
pub fn numeric_catalog() -> Catalog {
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![TableMetadata {
                schema: "public".to_string(),
                name: "metrics".to_string(),
                object_type: ObjectType::Table,
                description: None,
                columns: vec![
                    ColumnMetadata {
                        name: "amount".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "numeric"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "numeric".to_string(),
                        data_type: DataType::from_database_type("numeric"),
                        not_null: true,
                    },
                    ColumnMetadata {
                        name: "ratio".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "float8"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "float8".to_string(),
                        data_type: DataType::from_database_type("float8"),
                        not_null: false,
                    },
                    ColumnMetadata {
                        name: "enabled".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "bool"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "bool".to_string(),
                        data_type: DataType::from_database_type("bool"),
                        not_null: true,
                    },
                    ColumnMetadata {
                        name: "exists".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "int8"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "int8".to_string(),
                        data_type: DataType::from_database_type("int8"),
                        not_null: true,
                    },
                    ColumnMetadata {
                        name: "in".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "int8"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "int8".to_string(),
                        data_type: DataType::from_database_type("int8"),
                        not_null: true,
                    },
                    ColumnMetadata {
                        name: "is".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "int8"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "int8".to_string(),
                        data_type: DataType::from_database_type("int8"),
                        not_null: true,
                    },
                    ColumnMetadata {
                        name: "not".to_string(),
                        description: None,
                        provider_type: TypeKey::new("pg_catalog", "int8"),
                        formatted_type: None,
                        type_modifier: None,
                        database_type: "int8".to_string(),
                        data_type: DataType::from_database_type("int8"),
                        not_null: true,
                    },
                ],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("numeric fixture catalog must build")
    .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

/// Provider-backed scalar types plus one deliberately unsupported composite.
pub fn provider_scalar_catalog() -> Catalog {
    let type_metadata = |name: &str, kind: &str, category: &str| TypeMetadata {
        internal_type: name.to_string(),
        readable_type: format!("pg_catalog.{name}"),
        schema: "pg_catalog".to_string(),
        structure: TypeStructureMetadata::scalar(),
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
            .collect::<BTreeSet<_>>(),
    };
    let column = |name: &str, provider_name: &str| ColumnMetadata {
        name: name.to_string(),
        description: None,
        provider_type: TypeKey::new("pg_catalog", provider_name),
        formatted_type: None,
        type_modifier: None,
        database_type: provider_name.to_string(),
        data_type: DataType::from_database_type(provider_name),
        not_null: true,
    };
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![
                TableMetadata {
                    schema: "public".to_string(),
                    name: "events".to_string(),
                    object_type: ObjectType::Table,
                    description: None,
                    columns: vec![
                        column("event_date", "date"),
                        column("local_time", "timestamp"),
                        column("address", "inet"),
                        column("opaque", "opaque_record"),
                        column("short_label", "varchar"),
                        column("long_label", "text"),
                        column("small_id", "int4"),
                        column("big_id", "int8"),
                        column("active", "bool"),
                    ],
                    constraints: Vec::new(),
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                },
                TableMetadata {
                    schema: "public".to_string(),
                    name: "event_archive".to_string(),
                    object_type: ObjectType::Table,
                    description: None,
                    columns: vec![column("event_date", "date")],
                    constraints: Vec::new(),
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                },
            ],
        }],
        types: vec![
            type_metadata("date", "b", "D"),
            type_metadata("timestamp", "b", "D"),
            type_metadata("inet", "b", "I"),
            type_metadata("opaque_record", "c", "C"),
            type_metadata("varchar", "b", "S"),
            type_metadata("text", "b", "S"),
            type_metadata("int4", "b", "N"),
            type_metadata("int8", "b", "N"),
            type_metadata("bool", "b", "B"),
        ],
    }
    .to_catalog()
    .expect("provider scalar fixture catalog must build")
    .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

/// Provider domains and arrays used to pin structural type behavior.
pub fn structured_type_catalog() -> Catalog {
    let provider =
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
                        "addresses",
                        TypeKey::new("pg_catalog", "_inet"),
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
                    column(
                        "nested_domain_labels",
                        TypeKey::new("public", "_labels_domain"),
                        DataType::Unknown,
                    ),
                ],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: vec![
            provider(
                "pg_catalog",
                "text",
                "b",
                "S",
                TypeStructureMetadata::scalar(),
            ),
            provider(
                "pg_catalog",
                "int8",
                "b",
                "N",
                TypeStructureMetadata::scalar(),
            ),
            provider(
                "pg_catalog",
                "inet",
                "b",
                "I",
                TypeStructureMetadata::scalar(),
            ),
            provider(
                "public",
                "label_domain",
                "d",
                "S",
                TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "text")),
            ),
            provider(
                "public",
                "address_domain",
                "d",
                "I",
                TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "inet")),
            ),
            provider(
                "public",
                "nested_label_domain",
                "d",
                "S",
                TypeStructureMetadata::domain(TypeKey::new("public", "label_domain")),
            ),
            provider(
                "public",
                "labels_domain",
                "d",
                "A",
                TypeStructureMetadata::domain(TypeKey::new("pg_catalog", "_text")),
            ),
            provider(
                "pg_catalog",
                "_text",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("pg_catalog", "text")),
            ),
            provider(
                "pg_catalog",
                "_int8",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("pg_catalog", "int8")),
            ),
            provider(
                "pg_catalog",
                "_inet",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("pg_catalog", "inet")),
            ),
            provider(
                "public",
                "_label_domain",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("public", "label_domain")),
            ),
            provider(
                "public",
                "_labels_domain",
                "b",
                "A",
                TypeStructureMetadata::array(TypeKey::new("public", "labels_domain")),
            ),
        ],
    }
    .to_catalog()
    .expect("structured type fixture catalog must build")
    .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

/// Native enums, a domain over one enum, and an enum array.
pub fn native_enum_catalog() -> Catalog {
    let enum_type = |schema: &str, name: &str, description: Option<&str>| TypeMetadata {
        internal_type: name.to_string(),
        readable_type: format!("{schema}.{name}"),
        schema: schema.to_string(),
        structure: TypeStructureMetadata::enumeration(EnumTypeMetadata {
            description: description.map(str::to_string),
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
        |schema: &str, name: &str, kind: &str, category: &str, structure: TypeStructureMetadata| {
            TypeMetadata {
                internal_type: name.to_string(),
                readable_type: format!("{schema}.{name}"),
                schema: schema.to_string(),
                structure,
                provider: Some(ProviderTypeFacts {
                    kind: kind.to_string(),
                    category: category.to_string(),
                    effective_kind: None,
                    effective_category: None,
                    orderable: true,
                }),
                operations: ["=", "<>"].into_iter().map(str::to_string).collect(),
            }
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
                    column("audit_status", TypeKey::new("audit", "status")),
                    column("statuses", TypeKey::new("public", "_status")),
                ],
                constraints: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }],
        }],
        types: vec![
            enum_type(
                "public",
                "status",
                Some("Lifecycle status in provider order."),
            ),
            enum_type("audit", "status", Some("Independent audit status.")),
            provider_type(
                "public",
                "status_domain",
                "d",
                "E",
                TypeStructureMetadata::domain(status.clone()),
            ),
            provider_type(
                "public",
                "_status",
                "b",
                "A",
                TypeStructureMetadata::array(status),
            ),
        ],
    }
    .to_catalog()
    .expect("native enum fixture catalog must build")
    .with_default_schema(Catalog::DEFAULT_SCHEMA)
}

/// A catalog with repeated field names and differing logical types for
/// structural-policy completion coverage.
pub fn policy_completion_catalog() -> Catalog {
    let table = |name: &str, columns: &[(&str, DataType)]| TableMetadata {
        schema: "public".to_string(),
        name: name.to_string(),
        object_type: ObjectType::Table,
        description: None,
        columns: columns
            .iter()
            .map(|(name, data_type)| ColumnMetadata {
                name: (*name).to_string(),
                description: None,
                provider_type: TypeKey::new("pg_catalog", data_type.as_str()),
                formatted_type: None,
                type_modifier: None,
                database_type: data_type.as_str().to_string(),
                data_type: *data_type,
                not_null: true,
            })
            .collect(),
        constraints: Vec::new(),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
    };
    DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![
                table(
                    "first",
                    &[
                        ("nr_order", DataType::Int),
                        ("shared", DataType::Text),
                        ("only_first", DataType::Uuid),
                    ],
                ),
                table(
                    "second",
                    &[
                        ("nr_order", DataType::Text),
                        ("shared", DataType::Text),
                        ("only_second", DataType::Boolean),
                    ],
                ),
            ],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("policy completion fixture catalog must build")
    .with_default_schema(Catalog::DEFAULT_SCHEMA)
}
