use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Catalog {
    pub schemas: Vec<Schema>,
    pub tables: Vec<Table>,
    pub columns: Vec<Column>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct SchemaKey {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct TableKey {
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct ColumnKey {
    pub schema: String,
    pub table: String,
    pub column: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Schema {
    pub id: SchemaId,
    pub key: SchemaKey,
    pub name: String,
    pub tables: Vec<TableId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Table {
    pub id: TableId,
    pub key: TableKey,
    pub schema_id: SchemaId,
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnId>,
    pub primary_key: Vec<ColumnId>,
    pub foreign_keys_from: Vec<ForeignKeyId>,
    pub foreign_keys_to: Vec<ForeignKeyId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Column {
    pub id: ColumnId,
    pub key: ColumnKey,
    pub table: TableId,
    pub name: String,
    pub data_type: DataType,
    pub not_null: bool,
    pub is_unique: bool,
    pub is_indexed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct ForeignKey {
    pub id: ForeignKeyId,
    pub from_columns: Vec<ColumnId>,
    pub to_columns: Vec<ColumnId>,
    pub from_table: TableId,
    pub to_table: TableId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct SchemaId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct TableId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct ColumnId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(transparent)]
pub struct ForeignKeyId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(u8)]
pub enum DataType {
    Uuid,
    Text,
    Timestamptz,
    Int,
    Boolean,
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableResolution<'a> {
    Found(&'a Table),
    NotFound {
        reference: String,
    },
    Ambiguous {
        reference: String,
        candidates: Vec<TableKey>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldCheckResult<'a> {
    Column(&'a Column),
    Relation(RelationField<'a>),
    NotFound,
    AmbiguousRelation {
        reference: String,
        candidates: Vec<TableKey>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationField<'a> {
    pub name: &'a str,
    pub table: &'a Table,
    pub foreign_key: &'a ForeignKey,
}

impl Catalog {
    pub const DEFAULT_SCHEMA: &'static str = "public";

    pub fn hardcoded() -> Self {
        let public = SchemaId(0);
        let other_schema = SchemaId(1);
        let users = TableId(0);
        let posts = TableId(1);
        let other_users = TableId(2);

        let users_id = ColumnId(0);
        let users_name = ColumnId(1);
        let users_email = ColumnId(2);
        let users_created_at = ColumnId(3);

        let posts_id = ColumnId(4);
        let posts_title = ColumnId(5);
        let posts_body = ColumnId(6);
        let posts_user_id = ColumnId(7);
        let posts_created_at = ColumnId(8);
        let other_users_id = ColumnId(9);
        let other_users_name = ColumnId(10);

        let posts_user_fk = ForeignKeyId(0);

        Self {
            schemas: vec![
                Schema::new(public, "public", vec![users, posts]),
                Schema::new(other_schema, "other_schema", vec![other_users]),
            ],
            tables: vec![
                Table::new(
                    users,
                    public,
                    "public",
                    "users",
                    vec![users_id, users_name, users_email, users_created_at],
                    vec![users_id],
                    Vec::new(),
                    vec![posts_user_fk],
                ),
                Table::new(
                    posts,
                    public,
                    "public",
                    "posts",
                    vec![
                        posts_id,
                        posts_title,
                        posts_body,
                        posts_user_id,
                        posts_created_at,
                    ],
                    vec![posts_id],
                    vec![posts_user_fk],
                    Vec::new(),
                ),
                Table::new(
                    other_users,
                    other_schema,
                    "other_schema",
                    "users",
                    vec![other_users_id, other_users_name],
                    vec![other_users_id],
                    Vec::new(),
                    Vec::new(),
                ),
            ],
            columns: vec![
                Column::new(
                    users_id,
                    users,
                    "public",
                    "users",
                    "id",
                    DataType::Uuid,
                    true,
                    true,
                    true,
                ),
                Column::new(
                    users_name,
                    users,
                    "public",
                    "users",
                    "name",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    users_email,
                    users,
                    "public",
                    "users",
                    "email",
                    DataType::Text,
                    false,
                    false,
                    false,
                ),
                Column::new(
                    users_created_at,
                    users,
                    "public",
                    "users",
                    "created_at",
                    DataType::Timestamptz,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    posts_id,
                    posts,
                    "public",
                    "posts",
                    "id",
                    DataType::Uuid,
                    true,
                    true,
                    true,
                ),
                Column::new(
                    posts_title,
                    posts,
                    "public",
                    "posts",
                    "title",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    posts_body,
                    posts,
                    "public",
                    "posts",
                    "body",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    posts_user_id,
                    posts,
                    "public",
                    "posts",
                    "user_id",
                    DataType::Uuid,
                    true,
                    false,
                    true,
                ),
                Column::new(
                    posts_created_at,
                    posts,
                    "public",
                    "posts",
                    "created_at",
                    DataType::Timestamptz,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    other_users_id,
                    other_users,
                    "other_schema",
                    "users",
                    "id",
                    DataType::Uuid,
                    true,
                    true,
                    true,
                ),
                Column::new(
                    other_users_name,
                    other_users,
                    "other_schema",
                    "users",
                    "name",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
            ],
            foreign_keys: vec![ForeignKey {
                id: posts_user_fk,
                from_columns: vec![posts_user_id],
                to_columns: vec![users_id],
                from_table: posts,
                to_table: users,
            }],
        }
    }

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

        self.table(Self::DEFAULT_SCHEMA, name).map_or_else(
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
                    table: related,
                    foreign_key: fk,
                });
            }
        }
        relations
    }

    pub fn check_field(&self, table: TableId, field: &str) -> FieldCheckResult<'_> {
        let Some(table) = self.tables.get(table.0) else {
            return FieldCheckResult::NotFound;
        };

        let (field_schema, field_name) = split_schema_ref(field);
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
                    && relation.table.schema == field_schema.unwrap_or(Self::DEFAULT_SCHEMA)
            })
            .collect::<Vec<_>>();
        match relation_candidates.as_slice() {
            [] => FieldCheckResult::NotFound,
            [relation] => FieldCheckResult::Relation(*relation),
            _ => FieldCheckResult::AmbiguousRelation {
                reference: field.to_string(),
                candidates: relation_candidates
                    .iter()
                    .map(|relation| relation.table.key.clone())
                    .collect(),
            },
        }
    }
}

fn split_schema_ref(reference: &str) -> (Option<&str>, &str) {
    reference
        .split_once('.')
        .map_or((None, reference), |(schema, name)| (Some(schema), name))
}

impl Schema {
    pub fn new(id: SchemaId, name: &str, tables: Vec<TableId>) -> Self {
        Self {
            id,
            key: SchemaKey {
                name: name.to_string(),
            },
            name: name.to_string(),
            tables,
        }
    }
}

impl Table {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: TableId,
        schema_id: SchemaId,
        schema: &str,
        name: &str,
        columns: Vec<ColumnId>,
        primary_key: Vec<ColumnId>,
        foreign_keys_from: Vec<ForeignKeyId>,
        foreign_keys_to: Vec<ForeignKeyId>,
    ) -> Self {
        Self {
            id,
            key: TableKey {
                schema: schema.to_string(),
                table: name.to_string(),
            },
            schema_id,
            schema: schema.to_string(),
            name: name.to_string(),
            columns,
            primary_key,
            foreign_keys_from,
            foreign_keys_to,
        }
    }
}

impl Column {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ColumnId,
        table: TableId,
        schema: &str,
        table_name: &str,
        name: &str,
        data_type: DataType,
        not_null: bool,
        is_unique: bool,
        is_indexed: bool,
    ) -> Self {
        Self {
            id,
            key: ColumnKey {
                schema: schema.to_string(),
                table: table_name.to_string(),
                column: name.to_string(),
            },
            table,
            name: name.to_string(),
            data_type,
            not_null,
            is_unique,
            is_indexed,
        }
    }
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uuid => "uuid",
            Self::Text => "text",
            Self::Timestamptz => "timestamptz",
            Self::Int => "int",
            Self::Boolean => "boolean",
            Self::Json => "json",
        }
    }
}
