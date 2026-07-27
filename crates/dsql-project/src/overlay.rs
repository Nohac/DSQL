//! Versioned catalog-overlay loading and effective-catalog composition.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use dsql_core::catalog::{
    Catalog, CatalogSupport, CatalogSupportKind, ColumnId, DatabaseMetadata, ForeignKeyDirection,
    ForeignKeyId, ObjectType, Relation, RelationCardinality, RelationId, RelationSupports, TableId,
    TableMetadata, UniquenessSupport,
};
use facet::Facet;
use tokio::fs::{read_dir, read_to_string};

use crate::ProjectError;
use crate::config::Result;

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayDocument {
    version: u64,
    #[facet(default)]
    objects: Vec<OverlayObject>,
}

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayObject {
    target: OverlayObjectRef,
    #[facet(default)]
    description: Option<String>,
    #[facet(default)]
    hidden: Option<bool>,
    #[facet(default)]
    columns: Vec<OverlayColumn>,
    #[facet(default)]
    assert_unique: Vec<OverlayUnique>,
    #[facet(default)]
    relationships: Vec<OverlayRelationship>,
    #[facet(default)]
    hide: OverlayHide,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayObjectRef {
    schema: String,
    name: String,
}

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayColumn {
    name: String,
    #[facet(default)]
    description: Option<String>,
    #[facet(default)]
    hidden: Option<bool>,
}

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayUnique {
    name: String,
    columns: Vec<String>,
}

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayRelationship {
    name: String,
    target: OverlayObjectRef,
    columns: Vec<OverlayRelationshipColumn>,
}

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayRelationshipColumn {
    local: String,
    target: String,
}

#[derive(Clone, Debug, Default, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayHide {
    #[facet(default)]
    relationships: Vec<OverlayRelationshipHide>,
}

#[derive(Clone, Debug, Facet)]
#[facet(deny_unknown_fields)]
struct OverlayRelationshipHide {
    target: OverlayObjectRef,
    selector: String,
    direction: OverlayRelationshipDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
enum OverlayRelationshipDirection {
    Referencing,
    Referenced,
}

struct LoadedOverlay {
    path: PathBuf,
    document: OverlayDocument,
}

/// Builds the one effective catalog consumed by every compiler subsystem.
pub(crate) async fn load_effective_catalog(
    metadata: &DatabaseMetadata,
    schema_dir: &Path,
    overlays_dir: &Path,
) -> Result<Catalog> {
    let overlays = load_overlays(overlays_dir).await?;
    let mut catalog = metadata.to_catalog().map_err(ProjectError::CatalogBuild)?;
    attach_provider_supports(metadata, schema_dir, &mut catalog);
    compose(metadata, &overlays, &mut catalog)?;
    Ok(catalog)
}

async fn load_overlays(root: &Path) -> Result<Vec<LoadedOverlay>> {
    let mut directories = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(directory) = directories.pop() {
        let mut entries = match read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && directory == root => {
                return Ok(Vec::new());
            }
            Err(source) => {
                return Err(ProjectError::Read {
                    path: directory,
                    source,
                });
            }
        };
        loop {
            let entry = entries
                .next_entry()
                .await
                .map_err(|source| ProjectError::Read {
                    path: directory.clone(),
                    source,
                })?;
            let Some(entry) = entry else {
                break;
            };
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .map_err(|source| ProjectError::Read {
                    path: path.clone(),
                    source,
                })?;
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|extension| extension.to_str()) == Some("yaml")
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut overlays = Vec::with_capacity(paths.len());
    for path in paths {
        let raw = read_to_string(&path)
            .await
            .map_err(|source| ProjectError::Read {
                path: path.clone(),
                source,
            })?;
        let document: OverlayDocument =
            facet_yaml::from_str(&raw).map_err(|error| ProjectError::Parse {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if document.version != 1 {
            return overlay_error(
                &path,
                "version",
                format!("unsupported catalog overlay version {}", document.version),
            );
        }
        overlays.push(LoadedOverlay { path, document });
    }
    Ok(overlays)
}

fn compose(
    metadata: &DatabaseMetadata,
    overlays: &[LoadedOverlay],
    catalog: &mut Catalog,
) -> Result<()> {
    let provider_tables = provider_table_map(metadata);
    let mut owners = BTreeMap::<String, (PathBuf, String)>::new();

    for overlay in overlays {
        for (object_index, patch) in overlay.document.objects.iter().enumerate() {
            let item = format!("objects[{object_index}]");
            let table =
                table_id(catalog, &patch.target).ok_or_else(|| ProjectError::CatalogOverlay {
                    path: overlay.path.clone(),
                    item_path: format!("{item}.target"),
                    message: format!(
                        "catalog object {}.{} was not found",
                        patch.target.schema, patch.target.name
                    ),
                })?;
            validate_object_shape(overlay, &item, patch)?;
            if patch.description.is_some() {
                claim(
                    &mut owners,
                    format!("object:{table:?}:description"),
                    &overlay.path,
                    &item,
                )?;
            }
            if patch.hidden.is_some() {
                claim(
                    &mut owners,
                    format!("object:{table:?}:hidden"),
                    &overlay.path,
                    &item,
                )?;
            }
            for (column_index, column) in patch.columns.iter().enumerate() {
                let column_item = format!("{item}.columns[{column_index}]");
                validate_column_shape(overlay, &column_item, column)?;
                let column_id = column_id(catalog, table, &column.name).ok_or_else(|| {
                    ProjectError::CatalogOverlay {
                        path: overlay.path.clone(),
                        item_path: column_item.clone(),
                        message: format!(
                            "column {}.{}.{} was not found",
                            patch.target.schema, patch.target.name, column.name
                        ),
                    }
                })?;
                if column.description.is_some() {
                    claim(
                        &mut owners,
                        format!("column:{column_id:?}:description"),
                        &overlay.path,
                        &column_item,
                    )?;
                }
                if column.hidden.is_some() {
                    claim(
                        &mut owners,
                        format!("column:{column_id:?}:hidden"),
                        &overlay.path,
                        &column_item,
                    )?;
                }
            }
            for (unique_index, unique) in patch.assert_unique.iter().enumerate() {
                claim(
                    &mut owners,
                    format!("unique:{table:?}:{}", unique.name),
                    &overlay.path,
                    &format!("{item}.assert_unique[{unique_index}]"),
                )?;
            }
            for (relationship_index, relationship) in patch.relationships.iter().enumerate() {
                claim(
                    &mut owners,
                    format!("relation:{table:?}:{}", relationship.name),
                    &overlay.path,
                    &format!("{item}.relationships[{relationship_index}]"),
                )?;
            }
            for (hide_index, hide) in patch.hide.relationships.iter().enumerate() {
                claim(
                    &mut owners,
                    format!(
                        "hide:{table:?}:{}:{}:{}:{:?}",
                        hide.target.schema, hide.target.name, hide.selector, hide.direction
                    ),
                    &overlay.path,
                    &format!("{item}.hide.relationships[{hide_index}]"),
                )?;
            }
        }
    }

    for overlay in overlays {
        for (object_index, patch) in overlay.document.objects.iter().enumerate() {
            let item = format!("objects[{object_index}]");
            let Some(table) = table_id(catalog, &patch.target) else {
                continue;
            };
            apply_object_patch(catalog, overlay, table, patch);
            apply_uniqueness(
                catalog,
                provider_tables.get(&patch.target),
                overlay,
                &item,
                table,
                patch,
            )?;
        }
    }
    for overlay in overlays {
        for (object_index, patch) in overlay.document.objects.iter().enumerate() {
            let item = format!("objects[{object_index}]");
            let Some(table) = table_id(catalog, &patch.target) else {
                continue;
            };
            apply_relation_hides(catalog, overlay, &item, table, patch)?;
        }
    }
    for overlay in overlays {
        for (object_index, patch) in overlay.document.objects.iter().enumerate() {
            let item = format!("objects[{object_index}]");
            let Some(table) = table_id(catalog, &patch.target) else {
                continue;
            };
            apply_relationships(catalog, overlay, &item, table, patch)?;
        }
    }
    validate_effective_graph(overlays, catalog)
}

fn validate_object_shape(overlay: &LoadedOverlay, item: &str, patch: &OverlayObject) -> Result<()> {
    if patch.hidden == Some(false) {
        return overlay_error(
            &overlay.path,
            format!("{item}.hidden"),
            "`hidden: false` is not a version-1 overlay operation",
        );
    }
    if patch.hidden == Some(true)
        && (patch.description.is_some()
            || !patch.columns.is_empty()
            || !patch.assert_unique.is_empty()
            || !patch.relationships.is_empty()
            || !patch.hide.relationships.is_empty())
    {
        return overlay_error(
            &overlay.path,
            item,
            "an object hide cannot be combined with another patch",
        );
    }
    Ok(())
}

fn validate_column_shape(overlay: &LoadedOverlay, item: &str, patch: &OverlayColumn) -> Result<()> {
    if patch.hidden == Some(false) {
        return overlay_error(
            &overlay.path,
            format!("{item}.hidden"),
            "`hidden: false` is not a version-1 overlay operation",
        );
    }
    if patch.hidden == Some(true) && patch.description.is_some() {
        return overlay_error(
            &overlay.path,
            item,
            "a column hide cannot be combined with a description override",
        );
    }
    Ok(())
}

fn apply_object_patch(
    catalog: &mut Catalog,
    overlay: &LoadedOverlay,
    table: TableId,
    patch: &OverlayObject,
) {
    let support_item = overlay_object_item(&patch.target);
    let support = |suffix: &str| overlay_support(&overlay.path, format!("{support_item}.{suffix}"));
    if let Some(description) = &patch.description {
        catalog.tables[table.0].description = Some(description.clone());
        catalog.tables[table.0].description_support = Some(support("description"));
    }
    if patch.hidden == Some(true) {
        catalog.tables[table.0].visible = false;
        catalog.tables[table.0]
            .exposure_support
            .push(support("hidden"));
    }
    for patch in &patch.columns {
        let Some(column) = column_id(catalog, table, &patch.name) else {
            continue;
        };
        if let Some(description) = &patch.description {
            catalog.columns[column.0].description = Some(description.clone());
            catalog.columns[column.0].description_support =
                Some(support(&format!("columns[{}].description", patch.name)));
        }
        if patch.hidden == Some(true) {
            catalog.columns[column.0].visible = false;
            catalog.columns[column.0]
                .exposure_support
                .push(support(&format!("columns[{}].hidden", patch.name)));
        }
    }
}

fn apply_uniqueness(
    catalog: &mut Catalog,
    provider: Option<&&TableMetadata>,
    overlay: &LoadedOverlay,
    item: &str,
    table: TableId,
    patch: &OverlayObject,
) -> Result<()> {
    for (unique_index, unique) in patch.assert_unique.iter().enumerate() {
        let unique_item = format!("{item}.assert_unique[{unique_index}]");
        if !matches!(
            catalog.tables[table.0].object_type,
            ObjectType::View | ObjectType::MaterializedView
        ) {
            return overlay_error(
                &overlay.path,
                &unique_item,
                "`assert_unique` is allowed only on views and materialized views",
            );
        }
        if unique.name.is_empty() || unique.columns.is_empty() {
            return overlay_error(
                &overlay.path,
                &unique_item,
                "uniqueness assertion name and columns must be non-empty",
            );
        }
        let columns = unique
            .columns
            .iter()
            .map(|name| {
                column_id(catalog, table, name).ok_or_else(|| ProjectError::CatalogOverlay {
                    path: overlay.path.clone(),
                    item_path: unique_item.clone(),
                    message: format!("uniqueness assertion references missing column {name}"),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut distinct_columns = HashSet::new();
        if columns
            .iter()
            .any(|column| !distinct_columns.insert(*column))
        {
            return overlay_error(
                &overlay.path,
                &unique_item,
                "uniqueness assertion repeats a column",
            );
        }
        if provider.is_some_and(|provider| {
            provider
                .constraints
                .iter()
                .any(|constraint| constraint.name.as_deref() == Some(&unique.name))
                || provider
                    .indexes
                    .iter()
                    .any(|index| index.name.as_deref() == Some(&unique.name))
        }) {
            return overlay_error(
                &overlay.path,
                &unique_item,
                format!(
                    "uniqueness assertion name `{}` collides with provider metadata",
                    unique.name
                ),
            );
        }
        if catalog
            .uniqueness_supports
            .iter()
            .any(|support| support.table == table && support.columns == columns)
        {
            return overlay_error(
                &overlay.path,
                &unique_item,
                "uniqueness assertion repeats an existing ordered column tuple",
            );
        }
        catalog.tables[table.0]
            .unique_constraints
            .push(columns.clone());
        let support = overlay_support(
            &overlay.path,
            format!(
                "{}.assert_unique[{}]",
                overlay_object_item(&patch.target),
                unique.name
            ),
        );
        catalog.uniqueness_supports.push(UniquenessSupport {
            table,
            name: unique.name.clone(),
            columns,
            support,
        });
    }
    Ok(())
}

fn apply_relation_hides(
    catalog: &mut Catalog,
    overlay: &LoadedOverlay,
    item: &str,
    table: TableId,
    patch: &OverlayObject,
) -> Result<()> {
    for (hide_index, hide) in patch.hide.relationships.iter().enumerate() {
        let hide_item = format!("{item}.hide.relationships[{hide_index}]");
        let Some(target) = table_id(catalog, &hide.target) else {
            return overlay_error(
                &overlay.path,
                &hide_item,
                format!(
                    "relationship hide target {}.{} was not found",
                    hide.target.schema, hide.target.name
                ),
            );
        };
        if !catalog.tables[target.0].visible {
            return overlay_error(
                &overlay.path,
                &hide_item,
                "provider relationship target is already hidden",
            );
        }
        let matches = catalog.tables[table.0]
            .relations
            .iter()
            .filter_map(|relation| catalog.relations.get(relation.0))
            .filter(|relation| {
                relation.visible
                    && relation.to_table == target
                    && relation.selector == hide.selector
                    && relation.join_support.is_some()
                    && relation.join_direction == Some(hide.direction.foreign_key_direction())
            })
            .map(|relation| relation.id)
            .collect::<Vec<_>>();
        let [relation] = matches.as_slice() else {
            return overlay_error(
                &overlay.path,
                &hide_item,
                if matches.is_empty() {
                    "provider relationship hide target was not found".to_string()
                } else {
                    "provider relationship hide target is ambiguous".to_string()
                },
            );
        };
        catalog.relations[relation.0].visible = false;
        catalog.relations[relation.0]
            .supports
            .exposure
            .push(overlay_support(
                &overlay.path,
                format!(
                    "{}.hide.relationships[{}:{}:{}:{}]",
                    overlay_object_item(&patch.target),
                    hide.target.schema,
                    hide.target.name,
                    hide.selector,
                    hide.direction.as_str()
                ),
            ));
    }
    Ok(())
}

fn apply_relationships(
    catalog: &mut Catalog,
    overlay: &LoadedOverlay,
    item: &str,
    table: TableId,
    patch: &OverlayObject,
) -> Result<()> {
    for (relationship_index, relationship) in patch.relationships.iter().enumerate() {
        let relationship_item = format!("{item}.relationships[{relationship_index}]");
        if !is_dsql_name(&relationship.name) || relationship.columns.is_empty() {
            return overlay_error(
                &overlay.path,
                &relationship_item,
                "relationship name must be a valid DSQL name and columns must be non-empty",
            );
        }
        let Some(target) = table_id(catalog, &relationship.target) else {
            return overlay_error(
                &overlay.path,
                &relationship_item,
                format!(
                    "relationship target {}.{} was not found",
                    relationship.target.schema, relationship.target.name
                ),
            );
        };
        if !catalog.tables[target.0].visible {
            return overlay_error(
                &overlay.path,
                &relationship_item,
                "visible relationship cannot target a hidden object",
            );
        }
        if catalog
            .columns_for_table(table)
            .any(|column| column.name == relationship.name)
            || catalog
                .relation_fields_for_table(table)
                .iter()
                .any(|relation| relation.name == relationship.name)
        {
            return overlay_error(
                &overlay.path,
                &relationship_item,
                format!(
                    "relationship name `{}` collides in the effective field namespace",
                    relationship.name
                ),
            );
        }
        let mut local_columns = Vec::new();
        let mut target_columns = Vec::new();
        let mut local_seen = HashSet::new();
        let mut target_seen = HashSet::new();
        for (column_index, mapping) in relationship.columns.iter().enumerate() {
            let mapping_item = format!("{relationship_item}.columns[{column_index}]");
            let Some(local) = column_id(catalog, table, &mapping.local) else {
                return overlay_error(
                    &overlay.path,
                    &mapping_item,
                    format!("local column `{}` was not found", mapping.local),
                );
            };
            let Some(target_column) = column_id(catalog, target, &mapping.target) else {
                return overlay_error(
                    &overlay.path,
                    &mapping_item,
                    format!("target column `{}` was not found", mapping.target),
                );
            };
            if !local_seen.insert(local) || !target_seen.insert(target_column) {
                return overlay_error(
                    &overlay.path,
                    &mapping_item,
                    "relationship mapping repeats a local or target column",
                );
            }
            validate_column_types(catalog, overlay, &mapping_item, local, target_column)?;
            local_columns.push(local);
            target_columns.push(target_column);
        }
        let matching_foreign_keys =
            matching_foreign_keys(catalog, table, target, &local_columns, &target_columns);
        let (join_support, join_direction) = match matching_foreign_keys.as_slice() {
            [] => (None, None),
            [(foreign_key, direction)] => (Some(*foreign_key), Some(*direction)),
            _ => {
                return overlay_error(
                    &overlay.path,
                    &relationship_item,
                    "relationship mapping matches more than one provider foreign-key direction",
                );
            }
        };
        let cardinality = if catalog.column_set_is_unique(target, &target_columns) {
            RelationCardinality::Singular
        } else {
            RelationCardinality::Collection
        };
        let nullable = !(join_direction == Some(ForeignKeyDirection::Referencing)
            && local_columns
                .iter()
                .all(|column| catalog.columns[column.0].not_null));
        let support = overlay_support(
            &overlay.path,
            format!(
                "{}.relationships[{}]",
                overlay_object_item(&patch.target),
                relationship.name
            ),
        );
        let mut supports = RelationSupports {
            declaration: Some(support.clone()),
            ..RelationSupports::default()
        };
        if let Some(foreign_key) = join_support
            && let Some(provider) = provider_foreign_key_support(catalog, foreign_key)
        {
            supports.join.push(provider.clone());
            if !nullable {
                supports.presence.push(provider);
                supports.presence.extend(
                    local_columns
                        .iter()
                        .filter_map(|column| catalog.columns[column.0].declaration.clone()),
                );
            }
        }
        if cardinality == RelationCardinality::Singular {
            supports.cardinality = uniqueness_supports_for(catalog, target, &target_columns);
        }
        let relation = RelationId(catalog.relations.len());
        let selector = local_columns
            .iter()
            .map(|column| catalog.columns[column.0].name.as_str())
            .collect::<Vec<_>>()
            .join("_");
        catalog.relations.push(Relation {
            id: relation,
            name: relationship.name.clone(),
            selector,
            visible: true,
            from_table: table,
            to_table: target,
            local_columns,
            target_columns,
            cardinality,
            nullable,
            join_support,
            join_direction,
            supports,
        });
        catalog.tables[table.0].relations.push(relation);
    }
    Ok(())
}

fn validate_effective_graph(overlays: &[LoadedOverlay], catalog: &Catalog) -> Result<()> {
    for (overlay_index, overlay) in overlays.iter().enumerate() {
        for (object_index, patch) in overlay.document.objects.iter().enumerate() {
            let Some(table) = table_id(catalog, &patch.target) else {
                continue;
            };
            if !catalog.tables[table.0].visible {
                let has_other_patch =
                    overlays
                        .iter()
                        .enumerate()
                        .any(|(candidate_overlay_index, candidate)| {
                            candidate.document.objects.iter().enumerate().any(
                                |(candidate_object_index, other)| {
                                    other.target == patch.target
                                        && (candidate_overlay_index != overlay_index
                                            || candidate_object_index != object_index)
                                },
                            )
                        });
                if has_other_patch {
                    return overlay_error(
                        &overlay.path,
                        format!("objects[{object_index}]"),
                        "another overlay patch targets this hidden object",
                    );
                }
            }
            for (column_index, column) in patch.columns.iter().enumerate() {
                let Some(column_id) = column_id(catalog, table, &column.name) else {
                    continue;
                };
                if !catalog.columns[column_id.0].visible {
                    let patched_elsewhere =
                        overlays
                            .iter()
                            .enumerate()
                            .any(|(candidate_overlay_index, candidate)| {
                                candidate.document.objects.iter().enumerate().any(
                                    |(candidate_object_index, other)| {
                                        other.target == patch.target
                                            && other.columns.iter().enumerate().any(
                                                |(candidate_column_index, other_column)| {
                                                    other_column.name == column.name
                                                        && (candidate_overlay_index
                                                            != overlay_index
                                                            || candidate_object_index
                                                                != object_index
                                                            || candidate_column_index
                                                                != column_index)
                                                },
                                            )
                                    },
                                )
                            });
                    if patched_elsewhere {
                        return overlay_error(
                            &overlay.path,
                            format!("objects[{object_index}].columns[{column_index}]"),
                            "another overlay patch targets this hidden column",
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_column_types(
    catalog: &Catalog,
    overlay: &LoadedOverlay,
    item: &str,
    local: ColumnId,
    target: ColumnId,
) -> Result<()> {
    let local_column = &catalog.columns[local.0];
    let target_column = &catalog.columns[target.0];
    if local_column.type_id != target_column.type_id {
        let local_type = &catalog.types[local_column.type_id.0];
        let target_type = &catalog.types[target_column.type_id.0];
        return overlay_error(
            &overlay.path,
            item,
            format!(
                "relationship columns have incompatible types {}.{} and {}.{}",
                local_type.key.schema,
                local_type.key.name,
                target_type.key.schema,
                target_type.key.name
            ),
        );
    }
    Ok(())
}

fn attach_provider_supports(metadata: &DatabaseMetadata, schema_dir: &Path, catalog: &mut Catalog) {
    for schema in &metadata.schemas {
        for table_metadata in &schema.tables {
            let schema_name = if table_metadata.schema.is_empty() {
                &schema.name
            } else {
                &table_metadata.schema
            };
            let Some(table) = catalog
                .table(schema_name, &table_metadata.name)
                .map(|table| table.id)
            else {
                continue;
            };
            let path = schema_dir
                .join(schema_name)
                .join(format!("{}.yaml", table_metadata.name));
            catalog.tables[table.0].declaration =
                Some(provider_support(&path, "object".to_string()));
            if table_metadata.description.is_some() {
                catalog.tables[table.0].description_support =
                    Some(provider_support(&path, "description".to_string()));
            }
            for column_metadata in &table_metadata.columns {
                let Some(column) = column_id(catalog, table, &column_metadata.name) else {
                    continue;
                };
                catalog.columns[column.0].declaration = Some(provider_support(
                    &path,
                    format!("columns[{}]", column_metadata.name),
                ));
                if column_metadata.description.is_some() {
                    catalog.columns[column.0].description_support = Some(provider_support(
                        &path,
                        format!("columns[{}].description", column_metadata.name),
                    ));
                }
            }
            for constraint in &table_metadata.constraints {
                if !matches!(
                    constraint.kind,
                    dsql_core::catalog::TableConstraintKind::PrimaryKey
                        | dsql_core::catalog::TableConstraintKind::Unique
                ) {
                    continue;
                }
                let columns = constraint
                    .columns
                    .iter()
                    .filter_map(|column| column_id(catalog, table, column))
                    .collect::<Vec<_>>();
                let name = constraint
                    .name
                    .clone()
                    .unwrap_or_else(|| constraint.kind.as_ref().to_string());
                catalog.uniqueness_supports.push(UniquenessSupport {
                    table,
                    name: name.clone(),
                    columns,
                    support: provider_support(&path, format!("constraints[{name}]")),
                });
            }
            for index in &table_metadata.indexes {
                if !index.unique {
                    continue;
                }
                let columns = index
                    .keys
                    .iter()
                    .filter_map(|key| column_id(catalog, table, &key.column))
                    .collect::<Vec<_>>();
                let name = index.name.clone().unwrap_or_else(|| "index".to_string());
                catalog.uniqueness_supports.push(UniquenessSupport {
                    table,
                    name: name.clone(),
                    columns,
                    support: provider_support(&path, format!("indexes[{name}]")),
                });
            }
        }
    }
    for relation_index in 0..catalog.relations.len() {
        let Some(foreign_key_id) = catalog.relations[relation_index].join_support else {
            continue;
        };
        let foreign_key = &catalog.foreign_keys[foreign_key_id.0];
        let table = &catalog.tables[foreign_key.from_table.0];
        let path = schema_dir
            .join(&table.schema)
            .join(format!("{}.yaml", table.name));
        let name = foreign_key
            .name
            .clone()
            .unwrap_or_else(|| catalog.relations[relation_index].selector.clone());
        let support = provider_support(&path, format!("foreign_keys[{name}]"));
        let cardinality_supports =
            if catalog.relations[relation_index].cardinality == RelationCardinality::Singular {
                uniqueness_supports_for(
                    catalog,
                    catalog.relations[relation_index].to_table,
                    &catalog.relations[relation_index].target_columns,
                )
            } else {
                Vec::new()
            };
        let presence_supports = if !catalog.relations[relation_index].nullable {
            catalog.relations[relation_index]
                .local_columns
                .iter()
                .filter_map(|column| catalog.columns[column.0].declaration.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let relation = &mut catalog.relations[relation_index];
        relation.supports.declaration = Some(support.clone());
        relation.supports.join.push(support.clone());
        relation.supports.cardinality = cardinality_supports;
        if !relation.nullable {
            relation.supports.presence.push(support);
            relation.supports.presence.extend(presence_supports);
        }
    }
}

fn provider_table_map(metadata: &DatabaseMetadata) -> BTreeMap<OverlayObjectRef, &TableMetadata> {
    metadata
        .schemas
        .iter()
        .flat_map(|schema| {
            schema.tables.iter().map(move |table| {
                (
                    OverlayObjectRef {
                        schema: if table.schema.is_empty() {
                            schema.name.clone()
                        } else {
                            table.schema.clone()
                        },
                        name: table.name.clone(),
                    },
                    table,
                )
            })
        })
        .collect()
}

fn matching_foreign_keys(
    catalog: &Catalog,
    table: TableId,
    target: TableId,
    local: &[ColumnId],
    target_columns: &[ColumnId],
) -> Vec<(ForeignKeyId, ForeignKeyDirection)> {
    let mut matches = Vec::new();
    for foreign_key in &catalog.foreign_keys {
        if foreign_key.from_table == table
            && foreign_key.to_table == target
            && foreign_key.from_columns == local
            && foreign_key.to_columns == target_columns
        {
            matches.push((foreign_key.id, ForeignKeyDirection::Referencing));
        }
        if foreign_key.to_table == table
            && foreign_key.from_table == target
            && foreign_key.to_columns == local
            && foreign_key.from_columns == target_columns
        {
            matches.push((foreign_key.id, ForeignKeyDirection::Referenced));
        }
    }
    matches
}

fn provider_foreign_key_support(
    catalog: &Catalog,
    foreign_key: ForeignKeyId,
) -> Option<CatalogSupport> {
    catalog
        .relations
        .iter()
        .find(|relation| relation.join_support == Some(foreign_key))
        .and_then(|relation| relation.supports.join.first())
        .cloned()
}

fn uniqueness_supports_for(
    catalog: &Catalog,
    table: TableId,
    columns: &[ColumnId],
) -> Vec<CatalogSupport> {
    catalog
        .uniqueness_supports
        .iter()
        .filter(|support| {
            support.table == table
                && !support.columns.is_empty()
                && support
                    .columns
                    .iter()
                    .all(|column| columns.contains(column))
        })
        .map(|support| support.support.clone())
        .collect()
}

fn table_id(catalog: &Catalog, target: &OverlayObjectRef) -> Option<TableId> {
    catalog
        .tables
        .iter()
        .find(|table| table.schema == target.schema && table.name == target.name)
        .map(|table| table.id)
}

fn column_id(catalog: &Catalog, table: TableId, name: &str) -> Option<ColumnId> {
    catalog.tables[table.0]
        .columns
        .iter()
        .find_map(|column| (catalog.columns[column.0].name == name).then_some(*column))
}

fn claim(
    owners: &mut BTreeMap<String, (PathBuf, String)>,
    key: String,
    path: &Path,
    item: &str,
) -> Result<()> {
    if let Some((first_path, first_item)) =
        owners.insert(key, (path.to_path_buf(), item.to_string()))
    {
        return overlay_error(
            path,
            item,
            format!(
                "overlay semantic key is already owned by {} {}",
                first_path.display(),
                first_item
            ),
        );
    }
    Ok(())
}

fn provider_support(path: &Path, item_path: String) -> CatalogSupport {
    CatalogSupport {
        kind: CatalogSupportKind::Provider,
        path: path.display().to_string(),
        item_path,
        range: None,
    }
}

fn overlay_support(path: &Path, item_path: String) -> CatalogSupport {
    CatalogSupport {
        kind: CatalogSupportKind::Overlay,
        path: path.display().to_string(),
        item_path,
        range: None,
    }
}

fn overlay_object_item(target: &OverlayObjectRef) -> String {
    format!("objects[{}.{}]", target.schema, target.name)
}

fn is_dsql_name(value: &str) -> bool {
    let mut diagnostics = Vec::new();
    let (tokens, spans) = dsql_core::grammar::lexer::tokenize(value, &mut diagnostics);
    matches!(
        (tokens.as_slice(), spans.as_slice(), diagnostics.as_slice()),
        ([token], [span], [])
            if dsql_core::grammar::parser::is_name_token(*token)
                && span.start == 0
                && span.end == value.len()
    )
}

impl OverlayRelationshipDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Referencing => "referencing",
            Self::Referenced => "referenced",
        }
    }

    fn foreign_key_direction(self) -> ForeignKeyDirection {
        match self {
            Self::Referencing => ForeignKeyDirection::Referencing,
            Self::Referenced => ForeignKeyDirection::Referenced,
        }
    }
}

fn overlay_error<T>(
    path: &Path,
    item_path: impl Into<String>,
    message: impl Into<String>,
) -> Result<T> {
    Err(ProjectError::CatalogOverlay {
        path: path.to_path_buf(),
        item_path: item_path.into(),
        message: message.into(),
    })
}
