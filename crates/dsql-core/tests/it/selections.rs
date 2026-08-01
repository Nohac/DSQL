//! Selection facts: field selections and fragment spreads lower into a flat
//! parent-keyed encoding of the selection tree, and spreads resolve to their
//! fragment definitions through the bound join.

use std::collections::HashMap;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{
    CatalogSnapshot, DatabaseMetadata, SchemaMetadata, insert_catalog, table_metadata_from_yaml,
    table_metadata_to_yaml,
};
use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::field_selection::{FieldBodyKind, FieldSel};
use dsql_core::entities::fragment_spread::{ResolvedSpread, SpreadDecl};
use dsql_core::facts::{ChildOf, DiagnosticsDemand, NodeKey};
use dsql_core::resolution::{
    ResolvedSelection, ResolvedSelectionLimit, SelectionCardinalityProof, SelectionTarget,
};
use dsql_core::source::insert_source;

use crate::{fixture, imdb_catalog, render_diagnostic_facts, set_source_text};

async fn language_bowl() -> Bowl {
    dsql_core::language_bowl().await
}

/// Reconstructs the selection tree from the flat parent-keyed facts — the
/// same way planning and services consume them.
async fn render_selection_tree(bowl: &Bowl) -> String {
    let defs = bowl.scoop::<Query<(Entity, &DefDecl, &NodeKey)>>().await;
    let fields = bowl
        .scoop::<Query<(Entity, &FieldSel, &NodeKey, &ChildOf)>>()
        .await;
    let spreads = bowl
        .scoop::<Query<(Entity, &SpreadDecl, &NodeKey, &ChildOf)>>()
        .await;

    enum Node<'a> {
        Field(&'a FieldSel),
        Spread(&'a SpreadDecl),
    }

    let mut children: Vec<(Entity, Entity, usize, Node<'_>)> = Vec::new();
    let field_rows = fields.collect();
    for (entity, field, _, parent) in &field_rows {
        children.push((parent.0, *entity, field.span.start, Node::Field(field)));
    }
    let spread_rows = spreads.collect();
    for (entity, spread, _, parent) in &spread_rows {
        children.push((parent.0, *entity, spread.span.start, Node::Spread(spread)));
    }
    children.sort_by_key(|(_, _, start, _)| *start);

    fn render(
        parent: Entity,
        children: &[(Entity, Entity, usize, Node<'_>)],
        depth: usize,
        out: &mut String,
    ) {
        for (candidate_parent, key, _, node) in children {
            if *candidate_parent != parent {
                continue;
            }
            let indent = "  ".repeat(depth);
            match node {
                Node::Field(field) => {
                    let flattened = if field.flattened { "..." } else { "" };
                    let alias = field
                        .alias
                        .as_deref()
                        .map(|alias| format!("{alias}: "))
                        .unwrap_or_default();
                    let path = field
                        .relation_path
                        .as_deref()
                        .map(|path| format!("->{path}"))
                        .unwrap_or_default();
                    let nested = match field.body {
                        FieldBodyKind::None => "",
                        FieldBodyKind::SelectionSet => " {...}",
                        FieldBodyKind::Transform => " | {...}",
                    };
                    out.push_str(&format!(
                        "{indent}{flattened}{alias}{}{path}{nested}\n",
                        field.name
                    ));
                }
                Node::Spread(spread) => {
                    out.push_str(&format!("{indent}...{}\n", spread.name));
                }
            }
            render(*key, children, depth + 1, out);
        }
    }

    let mut def_rows = defs.collect();
    def_rows.sort_by_key(|(_, decl, _)| decl.span.start);
    let mut out = String::new();
    for (def_entity, decl, _) in def_rows {
        out.push_str(&format!("{} {}\n", decl.kind, decl.name));
        render(def_entity, &children, 1, &mut out);
    }
    out
}

#[tokio::test]
async fn flattened_selections_lower_as_fields_while_plain_spreads_stay_spreads() {
    let bowl = language_bowl().await;
    insert_source(
        &bowl,
        "flattened.dsql",
        indoc::indoc! {r#"
            fragment Bits on users { id }
            query Flattened {
              users(limit 1) {
                ...Bits
                ...posts | aggregate { post_count: count }
              }
              ...public::users | aggregate { user_count: count }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_selection_tree(&bowl).await);
}

#[tokio::test]
async fn selections_lower_into_a_parent_keyed_tree() {
    let bowl = language_bowl().await;

    insert_source(
        &bowl,
        "valid/imdb-relation-path-selector.dsql",
        &fixture("valid/imdb-relation-path-selector.dsql"),
    )
    .await;
    insert_source(
        &bowl,
        "valid/imdb-fragment-spread.dsql",
        &fixture("valid/imdb-fragment-spread.dsql"),
    )
    .await;

    insta::assert_snapshot!(render_selection_tree(&bowl).await);
}

/// Renders spread resolutions by name, sorted for stability.
async fn render_resolutions(bowl: &Bowl) -> Vec<String> {
    let spreads = bowl.scoop::<Query<(Entity, &ResolvedSpread)>>().await;
    let defs = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let def_rows = defs.collect();

    let mut lines: Vec<String> = spreads
        .collect()
        .into_iter()
        .filter_map(|(_, resolved)| {
            let target = resolved.target.as_ref()?;
            let fragment = def_rows
                .iter()
                .find(|(entity, _)| *entity == target.fragment)
                .map(|(_, decl)| decl.name.as_str())
                .unwrap_or("<missing fragment>");
            Some(format!("...{} -> fragment {fragment}", resolved.name))
        })
        .collect();
    lines.sort();
    lines
}

async fn render_selection_shapes(bowl: &Bowl) -> String {
    let resolutions = bowl.scoop::<Query<(Entity, &ResolvedSelection)>>().await;
    let fields = bowl.scoop::<Query<(Entity, &FieldSel)>>().await;
    let output_names = fields
        .collect()
        .into_iter()
        .map(|(entity, field)| {
            (
                entity,
                field.alias.clone().unwrap_or_else(|| field.name.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let catalogs = bowl.scoop::<Query<(Entity, &CatalogSnapshot)>>().await;
    let catalog_rows = catalogs.collect();
    let Some((_, snapshot)) = catalog_rows.first() else {
        return String::new();
    };
    let catalog = snapshot.catalog();
    let mut rows = resolutions
        .collect()
        .into_iter()
        .filter_map(|(_, resolved)| {
            let shape = resolved.shape.as_ref()?;
            let target = match resolved.target {
                SelectionTarget::Table(_) => "table",
                SelectionTarget::Relation { .. } => "relation",
                SelectionTarget::Column(_) | SelectionTarget::Unresolved => return None,
            };
            let proof = match &shape.proof {
                Some(SelectionCardinalityProof::Relation) => "relation".to_string(),
                Some(SelectionCardinalityProof::LimitOne) => "limit 1".to_string(),
                Some(SelectionCardinalityProof::UniqueKey(columns)) => format!(
                    "unique({})",
                    columns
                        .iter()
                        .filter_map(|column| catalog.column_by_id(*column))
                        .map(|column| column.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => "none".to_string(),
            };
            let limit = match shape.limit {
                ResolvedSelectionLimit::None => "none".to_string(),
                ResolvedSelectionLimit::Literal { value, .. } => format!("literal {value}"),
                ResolvedSelectionLimit::Runtime { .. } => "runtime".to_string(),
            };
            let output_name = output_names
                .get(&resolved.field)
                .map_or(resolved.written.as_str(), String::as_str);
            Some((
                resolved.name_span.start,
                format!(
                    "{target} {output_name}: {:?} proof={proof} nullable={} limit={limit}",
                    shape.cardinality, shape.nullable,
                ),
            ))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(start, _)| *start);
    rows.into_iter()
        .map(|(_, row)| row)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn resolved_selection_shapes_cover_catalog_predicate_and_limit_proofs() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    insert_source(
        &bowl,
        "shapes.dsql",
        indoc::indoc! {r#"
            query Shapes(%value? = null %named? = null) {
              collection: title { id }
              literal: title(limit 1) { id }
              runtime: title(limit %count) { id }
              primary: title(where .id == %id) { id }
              anonymous: title(where .id == %) { id }
              equality_operator: title(where .id %key_op[==] %operator_id) { id }
              optional_operator_value: title(where .id %optional_op[==] %) { id }
              optional_operator_value_reversed: title(where % %optional_reverse_op[==] .id) { id }
              optional_named_value: title(where .id %named_op[==] %named) { id }
              optional_named_value_reversed: title(where %named %named_reverse_op[==] .id) { id }
              extra: title(where .id == 1 and .production_year > 2000) { id }
              different_literal_or: title(where (.id == 1 and .title == "a") or .id == 2) { id }
              same_literal_or: title(where (.id == 1 and .title == "a") or .id == 1) { id }
              different_variable_or: title(where .id == %left or .id == %right) { id }
              same_variable_or: title(where (.id == %same and .title == "a") or .id == %same) { id }
              anonymous_or: title(where .id == % or .id == %) { id }
              bypass_or: title(where .id == 1 or .title == "a") { id }
              null_key: title(where .id == null) { id }
              row_value: title(where .id == .kind_id) { id }
              parent: title(limit 1) {
                kind_type { id }
                latest_info: movie_info(limit 1) { id }
              }
            }
            query MandatoryOperatorValue {
              mandatory_operator_value: title(where .id %mandatory_op[==] %) { id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_selection_shapes(&bowl).await);
}

#[tokio::test]
async fn selection_shape_edits_rederive_without_stale_cardinality() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, imdb_catalog()).await;
    let file = insert_source(
        &bowl,
        "shape-edit.dsql",
        "query ShapeEdit { title(limit 2) { id } }",
    )
    .await;
    let collection = render_selection_shapes(&bowl).await;

    set_source_text(&bowl, file, "query ShapeEdit { title(limit 1) { id } }").await;
    let singular = render_selection_shapes(&bowl).await;

    set_source_text(&bowl, file, "query ShapeEdit { title(limit 2) { id } }").await;
    let restored = render_selection_shapes(&bowl).await;

    insta::assert_snapshot!(format!(
        "collection:\n{collection}\n\nsingular:\n{singular}\n\nrestored:\n{restored}"
    ));
}

#[tokio::test]
async fn composite_nullable_unique_keys_require_every_mandatory_equality() {
    let table = table_metadata_from_yaml(
        r#"---
schema: public
name: memberships
object_type: table
columns:
  - name: tenant_id
    provider_type:
      schema: pg_catalog
      name: int4
    database_type: int4
    data_type: int
    not_null: true
  - name: user_id
    provider_type:
      schema: pg_catalog
      name: int4
    database_type: int4
    data_type: int
    not_null: false
  - name: locale
    provider_type:
      schema: pg_catalog
      name: text
    database_type: text
    data_type: text
    not_null: true
constraints:
  - name: memberships_tenant_user_key
    kind: unique
    columns: [tenant_id, user_id]
foreign_keys: []
indexes: []
"#,
    )
    .expect("embedded table metadata parses");
    let catalog = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![table],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("embedded catalog builds");
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    insert_source(
        &bowl,
        "composite-shapes.dsql",
        indoc::indoc! {r#"
            query CompositeShapes {
              complete: memberships(where .tenant_id == %tenant and .user_id == %user) { locale }
              incomplete: memberships(where .tenant_id == %tenant) { locale }
              different_value_or: memberships(where (.tenant_id == 1 and .user_id == 2) or (.user_id == 3 and .tenant_id == 1)) { locale }
              same_values_or: memberships(where (.tenant_id == 1 and .user_id == 2) or (.user_id == 2 and .tenant_id == 1 and .locale == "en")) { locale }
              bypass_or: memberships(where (.tenant_id == 1 and .user_id == 2) or .tenant_id == 1) { locale }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_selection_shapes(&bowl).await);
}

#[tokio::test]
async fn unique_index_includes_do_not_prove_selection_cardinality() {
    let table = table_metadata_from_yaml(
        r#"---
schema: public
name: memberships
object_type: table
columns:
  - name: tenant_id
    provider_type:
      schema: pg_catalog
      name: int4
    database_type: int4
    data_type: int
    not_null: true
  - name: user_id
    provider_type:
      schema: pg_catalog
      name: int4
    database_type: int4
    data_type: int
    not_null: true
constraints: []
foreign_keys: []
indexes:
  - name: memberships_tenant_id_key
    access_method: btree
    keys:
      - column: tenant_id
        capabilities: [equality]
    included_columns: [user_id]
    unique: true
"#,
    )
    .expect("embedded table metadata parses");
    let catalog = DatabaseMetadata {
        schemas: vec![SchemaMetadata {
            name: "public".to_string(),
            tables: vec![table],
        }],
        types: Vec::new(),
    }
    .to_catalog()
    .expect("embedded catalog builds");
    let bowl = language_bowl().await;
    insert_catalog(&bowl, catalog).await;
    insert_source(
        &bowl,
        "included-cardinality.dsql",
        indoc::indoc! {r#"
            query IncludedCardinality {
              key: memberships(where .tenant_id == %tenant) { user_id }
              include: memberships(where .user_id == %user) { tenant_id }
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_selection_shapes(&bowl).await);
}

#[test]
fn catalog_descriptions_round_trip_through_schema_yaml() {
    let table = table_metadata_from_yaml(
        r#"---
schema: public
name: stations
object_type: table
description: Observation stations.
columns:
  - name: code
    description: Stable public station code.
    provider_type:
      schema: pg_catalog
      name: text
    database_type: text
    data_type: text
    not_null: true
constraints: []
foreign_keys: []
indexes: []
"#,
    )
    .expect("embedded table metadata parses");

    let yaml = table_metadata_to_yaml(&table).expect("table metadata serializes");
    let restored = table_metadata_from_yaml(&yaml).expect("serialized table metadata parses");

    assert_eq!(
        restored.description.as_deref(),
        Some("Observation stations.")
    );
    assert_eq!(
        restored.columns[0].description.as_deref(),
        Some("Stable public station code.")
    );
}

#[tokio::test]
async fn spreads_resolve_to_fragments_in_the_same_file() {
    let bowl = language_bowl().await;

    insert_source(
        &bowl,
        "valid/imdb-fragment-spread.dsql",
        &fixture("valid/imdb-fragment-spread.dsql"),
    )
    .await;

    assert_eq!(
        render_resolutions(&bowl).await,
        vec!["...TitleFields -> fragment TitleFields"],
    );
}

#[tokio::test]
async fn renaming_a_fragment_retires_the_resolution() {
    let bowl = language_bowl().await;

    let file = insert_source(
        &bowl,
        "rename.dsql",
        "fragment F on title {\n  id\n}\nquery Q {\n  title {\n    ...F\n  }\n}\n",
    )
    .await;
    assert_eq!(render_resolutions(&bowl).await.len(), 1);

    set_source_text(
        &bowl,
        file,
        "fragment G on title {\n  id\n}\nquery Q {\n  title {\n    ...F\n  }\n}\n",
    )
    .await;

    assert_eq!(
        render_resolutions(&bowl).await.len(),
        0,
        "resolution must retire with the renamed fragment"
    );

    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}
