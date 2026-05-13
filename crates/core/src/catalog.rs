use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Catalog {
    pub schemas: Vec<Schema>,
    pub tables: Vec<Table>,
    pub columns: Vec<Column>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<TableId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Table {
    pub id: TableId,
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
pub enum FieldCheckResult<'a> {
    Column(&'a Column),
    Relation(&'a Table),
    NotFound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationField<'a> {
    pub name: &'a str,
    pub table: &'a Table,
}

impl Catalog {
    pub const DEFAULT_SCHEMA: &'static str = "public";

    pub fn hardcoded() -> Self {
        let users = TableId(0);
        let posts = TableId(1);

        let users_id = ColumnId(0);
        let users_name = ColumnId(1);
        let users_email = ColumnId(2);
        let users_created_at = ColumnId(3);

        let posts_id = ColumnId(4);
        let posts_title = ColumnId(5);
        let posts_body = ColumnId(6);
        let posts_user_id = ColumnId(7);
        let posts_created_at = ColumnId(8);

        let posts_user_fk = ForeignKeyId(0);

        Self {
            schemas: vec![Schema {
                name: "public".to_string(),
                tables: vec![users, posts],
            }],
            tables: vec![
                Table {
                    id: users,
                    schema: "public".to_string(),
                    name: "users".to_string(),
                    columns: vec![users_id, users_name, users_email, users_created_at],
                    primary_key: vec![users_id],
                    foreign_keys_from: Vec::new(),
                    foreign_keys_to: vec![posts_user_fk],
                },
                Table {
                    id: posts,
                    schema: "public".to_string(),
                    name: "posts".to_string(),
                    columns: vec![
                        posts_id,
                        posts_title,
                        posts_body,
                        posts_user_id,
                        posts_created_at,
                    ],
                    primary_key: vec![posts_id],
                    foreign_keys_from: vec![posts_user_fk],
                    foreign_keys_to: Vec::new(),
                },
            ],
            columns: vec![
                Column::new(users_id, users, "id", DataType::Uuid, true, true, true),
                Column::new(
                    users_name,
                    users,
                    "name",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    users_email,
                    users,
                    "email",
                    DataType::Text,
                    false,
                    false,
                    false,
                ),
                Column::new(
                    users_created_at,
                    users,
                    "created_at",
                    DataType::Timestamptz,
                    true,
                    false,
                    false,
                ),
                Column::new(posts_id, posts, "id", DataType::Uuid, true, true, true),
                Column::new(
                    posts_title,
                    posts,
                    "title",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    posts_body,
                    posts,
                    "body",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    posts_user_id,
                    posts,
                    "user_id",
                    DataType::Uuid,
                    true,
                    false,
                    true,
                ),
                Column::new(
                    posts_created_at,
                    posts,
                    "created_at",
                    DataType::Timestamptz,
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
        let (schema, name) = split_schema_ref(reference);
        self.table(schema.unwrap_or(Self::DEFAULT_SCHEMA), name)
    }

    pub fn table_by_id(&self, id: TableId) -> Option<&Table> {
        self.tables.get(id.0)
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
                });
            }
        }
        relations
    }

    pub fn check_field(&self, table: TableId, field: &str) -> FieldCheckResult<'_> {
        let Some(table) = self.tables.get(table.0) else {
            return FieldCheckResult::NotFound;
        };

        if let Some(column) = table
            .columns
            .iter()
            .filter_map(|id| self.columns.get(id.0))
            .find(|column| column.name == field)
        {
            return FieldCheckResult::Column(column);
        }

        let (field_schema, field_name) = split_schema_ref(field);
        let field_schema = field_schema.unwrap_or(Self::DEFAULT_SCHEMA);
        for relation in self.relation_fields_for_table(table.id) {
            if relation.table.schema == field_schema && relation.name == field_name {
                return FieldCheckResult::Relation(relation.table);
            }
        }

        FieldCheckResult::NotFound
    }
}

fn split_schema_ref(reference: &str) -> (Option<&str>, &str) {
    reference
        .split_once('.')
        .map_or((None, reference), |(schema, name)| (Some(schema), name))
}

impl Column {
    fn new(
        id: ColumnId,
        table: TableId,
        name: &str,
        data_type: DataType,
        not_null: bool,
        is_unique: bool,
        is_indexed: bool,
    ) -> Self {
        Self {
            id,
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
