//! Catalog lookup behavior independent of the language pipeline.

use dsql_core::catalog::{Catalog, TableRef, TableResolution};

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
