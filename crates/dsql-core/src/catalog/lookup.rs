use super::{
    Catalog, CatalogType, Column, ColumnId, DataType, FieldCheckResult, FieldRef, Index, Relation,
    RelationField, RelationId, Table, TableId, TableRef, TableResolution, TypeCapabilities, TypeId,
};

impl Catalog {
    pub fn table(&self, schema: &str, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|table| table.visible && table.schema == schema && table.name == name)
    }

    pub fn table_ref_for(&self, reference: TableRef<'_>) -> Option<&Table> {
        match self.resolve_table_ref_for(reference) {
            TableResolution::Found(table) => Some(table),
            TableResolution::NotFound { .. } | TableResolution::Ambiguous { .. } => None,
        }
    }

    pub fn resolve_table_ref_for(&self, reference: TableRef<'_>) -> TableResolution<'_> {
        if let Some(schema) = reference.schema {
            return self.table(schema, reference.name).map_or_else(
                || TableResolution::NotFound {
                    reference: reference.display_text(),
                },
                TableResolution::Found,
            );
        }

        let matches = self
            .tables
            .iter()
            .filter(|table| table.visible && table.name == reference.name)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => TableResolution::NotFound {
                reference: reference.display_text(),
            },
            [table] => TableResolution::Found(table),
            _ => {
                let mut candidates = matches
                    .iter()
                    .map(|table| table.key.clone())
                    .collect::<Vec<_>>();
                candidates.sort_by(|left, right| {
                    left.schema
                        .cmp(&right.schema)
                        .then_with(|| left.table.cmp(&right.table))
                });
                TableResolution::Ambiguous {
                    reference: reference.display_text(),
                    candidates,
                }
            }
        }
    }

    pub fn table_by_id(&self, id: TableId) -> Option<&Table> {
        self.tables.get(id.0)
    }

    pub fn column_by_id(&self, id: ColumnId) -> Option<&Column> {
        self.columns.get(id.0)
    }

    /// Resolves one dense catalog type identity.
    pub fn type_by_id(&self, id: TypeId) -> Option<&CatalogType> {
        self.types.get(id.0)
    }

    /// Resolves the effective type record for one catalog column.
    pub fn type_for_column(&self, id: ColumnId) -> Option<&CatalogType> {
        self.column_by_id(id)
            .and_then(|column| self.type_by_id(column.type_id))
    }

    /// Resolves exact provider capabilities for one validated column.
    pub fn capabilities_for_column(&self, id: ColumnId) -> Option<&TypeCapabilities> {
        self.type_for_column(id)
            .map(|data_type| &data_type.capabilities)
    }

    /// Returns compiler fallback semantics for a synthetic logical value.
    ///
    /// Provider-backed values must use [`Catalog::capabilities_for_column`] so
    /// later introspected capability differences are not erased.
    pub fn builtin_capabilities(data_type: DataType) -> TypeCapabilities {
        TypeCapabilities::builtin(data_type)
    }

    /// Resolves a canonical DSQL type name or supported PostgreSQL alias.
    ///
    /// Arbitrary provider names are deliberately excluded: an unmapped type
    /// must remain unresolved rather than silently becoming `unknown`.
    pub fn resolve_logical_type_name(name: &str) -> Option<DataType> {
        super::capabilities::data_type_from_logical_name(name)
    }

    /// Returns the logical type for a validated catalog column.
    ///
    /// Catalog construction guarantees both dense identities are valid. An
    /// invalid identity is a catalog-construction bug and intentionally fails
    /// loudly instead of changing language behavior to [`DataType::Unknown`].
    pub fn data_type_for_column(&self, id: ColumnId) -> DataType {
        let column = &self.columns[id.0];
        self.types[column.type_id.0].data_type
    }

    pub fn relation_by_id(&self, id: RelationId) -> Option<&Relation> {
        self.relations.get(id.0)
    }

    pub fn columns_for_table(&self, table: TableId) -> impl Iterator<Item = &Column> {
        self.tables
            .get(table.0)
            .into_iter()
            .flat_map(|table| &table.columns)
            .filter_map(|id| self.columns.get(id.0))
            .filter(|column| column.visible)
    }

    pub fn visible_tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.iter().filter(|table| table.visible)
    }

    pub fn relations_for_table(&self, table: TableId) -> Vec<&Table> {
        self.relation_fields_for_table(table)
            .into_iter()
            .map(|relation| relation.table)
            .collect()
    }

    pub fn relation_fields_for_table(&self, table: TableId) -> Vec<RelationField<'_>> {
        let Some(table) = self.tables.get(table.0) else {
            return Vec::new();
        };
        table
            .relations
            .iter()
            .filter_map(|id| self.relations.get(id.0))
            .filter(|relation| {
                relation.visible
                    && self
                        .tables
                        .get(relation.to_table.0)
                        .is_some_and(|table| table.visible)
            })
            .filter_map(|relation| {
                let related = self.tables.get(relation.to_table.0)?;
                Some(RelationField {
                    name: relation.name.as_str(),
                    selector: relation.selector.clone(),
                    table: related,
                    relation,
                })
            })
            .collect()
    }

    pub fn column_set_is_unique(&self, table: TableId, columns: &[ColumnId]) -> bool {
        self.table_by_id(table).is_some_and(|table| {
            table
                .unique_constraints
                .iter()
                .any(|constraint| column_set_covers(columns, constraint))
                || table.indexes.iter().any(|index| {
                    index.is_unique
                        && !index.keys.is_empty()
                        && index.keys.iter().all(|key| columns.contains(&key.column))
                })
        })
    }

    /// Whether a column participates in any true index key.
    pub fn column_participates_in_index(&self, column: ColumnId) -> bool {
        self.column_by_id(column)
            .and_then(|column| self.table_by_id(column.table))
            .is_some_and(|table| {
                table
                    .indexes
                    .iter()
                    .any(|index| index.keys.iter().any(|key| key.column == column))
            })
    }

    /// Whether a column is independently addressable by a physical index.
    pub fn column_is_independently_indexed(&self, column: ColumnId) -> bool {
        self.column_by_id(column)
            .and_then(|column| self.table_by_id(column.table))
            .is_some_and(|table| {
                table.indexes.iter().any(|index| {
                    index.keys.iter().enumerate().any(|(position, key)| {
                        key.column == column && index_key_is_independently_usable(index, position)
                    })
                })
            })
    }

    /// Whether an independently usable text index key supports `like`.
    pub fn column_is_searchable(&self, column: ColumnId) -> bool {
        self.column_by_id(column)
            .filter(|column| {
                self.capabilities_for_column(column.id)
                    .is_some_and(|capabilities| {
                        capabilities.supports(crate::entities::expression::ComparisonOp::Like)
                    })
            })
            .and_then(|column| self.table_by_id(column.table))
            .is_some_and(|table| {
                table.indexes.iter().any(|index| {
                    index.keys.iter().enumerate().any(|(position, key)| {
                        key.column == column
                            && key.capabilities.contains(&super::IndexKeyCapability::Like)
                            && index_key_is_independently_usable(index, position)
                    })
                })
            })
    }

    pub fn check_field_ref(&self, table: TableId, field: FieldRef<'_>) -> FieldCheckResult<'_> {
        let Some(table) = self.tables.get(table.0) else {
            return FieldCheckResult::NotFound;
        };

        if field.target.schema.is_none()
            && field.selector.is_none()
            && let Some(column) = table
                .columns
                .iter()
                .filter_map(|id| self.columns.get(id.0))
                .find(|column| column.visible && column.name == field.target.name)
        {
            return FieldCheckResult::Column(column);
        }

        let field_schema = field.target.schema.unwrap_or(&self.default_schema);
        let field_selector = field.selector;
        let relation_candidates = self
            .relation_fields_for_table(table.id)
            .into_iter()
            .filter(|relation| {
                relation.relation.name == field.target.name
                    && relation.table.schema == field_schema
                    && field_selector.is_none_or(|selector| selector == relation.relation.selector)
            })
            .collect::<Vec<_>>();
        match relation_candidates.as_slice() {
            [] => FieldCheckResult::NotFound,
            [relation] => FieldCheckResult::Relation(relation.clone()),
            _ => FieldCheckResult::AmbiguousRelation {
                reference: field.display_text(),
                candidates: relation_candidates
                    .iter()
                    .map(|relation| {
                        format!(
                            "{}::{}->{}",
                            relation.table.schema,
                            relation.relation.name,
                            relation.relation.selector
                        )
                    })
                    .collect(),
            },
        }
    }
}

fn index_key_is_independently_usable(index: &Index, position: usize) -> bool {
    position == 0
        || matches!(
            index.access_method.as_str(),
            "gin" | "gist" | "spgist" | "brin" | "hash"
        )
}

fn column_set_covers(columns: &[ColumnId], unique_columns: &[ColumnId]) -> bool {
    !unique_columns.is_empty()
        && unique_columns
            .iter()
            .all(|unique_column| columns.contains(unique_column))
}
