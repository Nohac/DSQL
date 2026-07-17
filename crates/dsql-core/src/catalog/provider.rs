use super::{
    Catalog, Column, ColumnId, DataType, ForeignKey, ForeignKeyId, Schema, SchemaId, Table, TableId,
};

impl Catalog {
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
            default_schema: Self::DEFAULT_SCHEMA.to_string(),
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
                    vec![vec![users_id]],
                    Vec::new(),
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
                    vec![vec![posts_id]],
                    Vec::new(),
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
                    vec![vec![other_users_id]],
                    Vec::new(),
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
                    "uuid",
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
                    "text",
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
                    "text",
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
                    "timestamptz",
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
                    "uuid",
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
                    "text",
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
                    "text",
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
                    "uuid",
                    DataType::Uuid,
                    true,
                    false,
                    false,
                ),
                Column::new(
                    posts_created_at,
                    posts,
                    "public",
                    "posts",
                    "created_at",
                    "timestamptz",
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
                    "uuid",
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
                    "text",
                    DataType::Text,
                    true,
                    false,
                    false,
                ),
            ],
            foreign_keys: vec![ForeignKey {
                id: posts_user_fk,
                name: Some("posts_user_id_fkey".to_string()),
                from_columns: vec![posts_user_id],
                to_columns: vec![users_id],
                from_table: posts,
                to_table: users,
            }],
        }
    }
}
