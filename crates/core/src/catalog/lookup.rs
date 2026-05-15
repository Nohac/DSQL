use super::{
    Catalog, Column, ColumnId, FieldCheckResult, ForeignKey, ForeignKeyId, RelationField, Table,
    TableId, TableResolution,
};

impl Catalog {
    pub fn table(&self, schema: &str, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|table| table.schema == schema && table.name == name)
    }

    pub fn table_ref(&self, reference: &str) -> Option<&Table> {
        match self.resolve_table_ref(reference) {
            TableResolution::Found(table) => Some(table),
            TableResolution::NotFound { .. } | TableResolution::Ambiguous { .. } => None,
        }
    }

    pub fn resolve_table_ref(&self, reference: &str) -> TableResolution<'_> {
        let (schema, name) = split_schema_ref(reference);
        if let Some(schema) = schema {
            return self.table(schema, name).map_or_else(
                || TableResolution::NotFound {
                    reference: reference.to_string(),
                },
                TableResolution::Found,
            );
        }

        self.table(&self.default_schema, name).map_or_else(
            || TableResolution::NotFound {
                reference: reference.to_string(),
            },
            TableResolution::Found,
        )
    }

    pub fn table_by_id(&self, id: TableId) -> Option<&Table> {
        self.tables.get(id.0)
    }

    pub fn column_by_id(&self, id: ColumnId) -> Option<&Column> {
        self.columns.get(id.0)
    }

    pub fn foreign_key_by_id(&self, id: ForeignKeyId) -> Option<&ForeignKey> {
        self.foreign_keys.get(id.0)
    }

    pub fn columns_for_table(&self, table: TableId) -> impl Iterator<Item = &Column> {
        self.tables
            .get(table.0)
            .into_iter()
            .flat_map(|table| &table.columns)
            .filter_map(|id| self.columns.get(id.0))
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
        let mut relations = Vec::new();
        for fk in table
            .foreign_keys_to
            .iter()
            .filter_map(|id| self.foreign_keys.get(id.0))
        {
            if let Some(related) = self.tables.get(fk.from_table.0) {
                relations.push(RelationField {
                    name: related.name.as_str(),
                    selector: self.foreign_key_selector(fk),
                    table: related,
                    foreign_key: fk,
                });
            }
        }
        for fk in table
            .foreign_keys_from
            .iter()
            .filter_map(|id| self.foreign_keys.get(id.0))
        {
            if let Some(related) = self.tables.get(fk.to_table.0) {
                relations.push(RelationField {
                    name: related.name.as_str(),
                    selector: self.foreign_key_selector(fk),
                    table: related,
                    foreign_key: fk,
                });
            }
        }
        relations
    }

    pub fn foreign_key_selector(&self, foreign_key: &ForeignKey) -> String {
        foreign_key
            .from_columns
            .iter()
            .filter_map(|id| self.columns.get(id.0))
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
            .join("_")
    }

    pub fn check_field(&self, table: TableId, field: &str) -> FieldCheckResult<'_> {
        let Some(table) = self.tables.get(table.0) else {
            return FieldCheckResult::NotFound;
        };

        let (field_ref, field_selector) = split_relation_selector(field);
        let (field_schema, field_name) = split_schema_ref(field_ref);
        if field_schema.is_none()
            && let Some(column) = table
                .columns
                .iter()
                .filter_map(|id| self.columns.get(id.0))
                .find(|column| column.name == field_name)
        {
            return FieldCheckResult::Column(column);
        }

        let relation_candidates = self
            .relation_fields_for_table(table.id)
            .into_iter()
            .filter(|relation| {
                relation.name == field_name
                    && relation.table.schema == field_schema.unwrap_or(&self.default_schema)
                    && field_selector.is_none_or(|selector| selector == relation.selector)
            })
            .collect::<Vec<_>>();
        match relation_candidates.as_slice() {
            [] => FieldCheckResult::NotFound,
            [relation] => FieldCheckResult::Relation(relation.clone()),
            _ => FieldCheckResult::AmbiguousRelation {
                reference: field.to_string(),
                candidates: relation_candidates
                    .iter()
                    .map(|relation| {
                        format!(
                            "{}.{}::{}",
                            relation.table.schema, relation.name, relation.selector
                        )
                    })
                    .collect(),
            },
        }
    }
}

fn split_relation_selector(reference: &str) -> (&str, Option<&str>) {
    reference
        .split_once("::")
        .map_or((reference, None), |(relation, selector)| {
            (relation, Some(selector))
        })
}

fn split_schema_ref(reference: &str) -> (Option<&str>, &str) {
    reference
        .split_once('.')
        .map_or((None, reference), |(schema, name)| (Some(schema), name))
}
