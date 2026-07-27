use super::{
    Catalog, CatalogType, Column, ColumnId, DataType, ForeignKey, ForeignKeyDirection,
    ForeignKeyId, Index, IndexKey, IndexKeyCapability, IndexNullsPosition, IndexOrder,
    IndexOrderDirection, ObjectType, Relation, RelationCardinality, RelationId, RelationSupports,
    Schema, SchemaId, Table, TableId, TypeId, TypeKey,
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
        let uuid = TypeId(0);
        let text = TypeId(1);
        let timestamptz = TypeId(2);

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
                    ObjectType::Table,
                    None,
                    vec![users_id, users_name, users_email, users_created_at],
                    vec![users_id],
                    vec![vec![users_id]],
                    vec![
                        btree_index("users_pkey", users_id, true),
                        searchable_index("users_name_search_idx", users_name),
                    ],
                    vec![RelationId(1)],
                ),
                Table::new(
                    posts,
                    public,
                    "public",
                    "posts",
                    ObjectType::Table,
                    None,
                    vec![
                        posts_id,
                        posts_title,
                        posts_body,
                        posts_user_id,
                        posts_created_at,
                    ],
                    vec![posts_id],
                    vec![vec![posts_id]],
                    vec![btree_index("posts_pkey", posts_id, true)],
                    vec![RelationId(0)],
                ),
                Table::new(
                    other_users,
                    other_schema,
                    "other_schema",
                    "users",
                    ObjectType::Table,
                    None,
                    vec![other_users_id, other_users_name],
                    vec![other_users_id],
                    vec![vec![other_users_id]],
                    vec![btree_index("other_users_pkey", other_users_id, true)],
                    Vec::new(),
                ),
            ],
            types: vec![
                CatalogType::builtin(uuid, TypeKey::new("pg_catalog", "uuid"), DataType::Uuid),
                CatalogType::builtin(text, TypeKey::new("pg_catalog", "text"), DataType::Text),
                CatalogType::builtin(
                    timestamptz,
                    TypeKey::new("pg_catalog", "timestamptz"),
                    DataType::Timestamptz,
                ),
            ],
            columns: vec![
                Column::new(
                    users_id, users, "public", "users", "id", None, uuid, true, true,
                ),
                Column::new(
                    users_name, users, "public", "users", "name", None, text, true, false,
                ),
                Column::new(
                    users_email,
                    users,
                    "public",
                    "users",
                    "email",
                    None,
                    text,
                    false,
                    false,
                ),
                Column::new(
                    users_created_at,
                    users,
                    "public",
                    "users",
                    "created_at",
                    None,
                    timestamptz,
                    true,
                    false,
                ),
                Column::new(
                    posts_id, posts, "public", "posts", "id", None, uuid, true, true,
                ),
                Column::new(
                    posts_title,
                    posts,
                    "public",
                    "posts",
                    "title",
                    None,
                    text,
                    true,
                    false,
                ),
                Column::new(
                    posts_body, posts, "public", "posts", "body", None, text, true, false,
                ),
                Column::new(
                    posts_user_id,
                    posts,
                    "public",
                    "posts",
                    "user_id",
                    None,
                    uuid,
                    true,
                    false,
                ),
                Column::new(
                    posts_created_at,
                    posts,
                    "public",
                    "posts",
                    "created_at",
                    None,
                    timestamptz,
                    true,
                    false,
                ),
                Column::new(
                    other_users_id,
                    other_users,
                    "other_schema",
                    "users",
                    "id",
                    None,
                    uuid,
                    true,
                    true,
                ),
                Column::new(
                    other_users_name,
                    other_users,
                    "other_schema",
                    "users",
                    "name",
                    None,
                    text,
                    true,
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
            relations: vec![
                Relation {
                    id: RelationId(0),
                    name: "users".to_string(),
                    selector: "user_id".to_string(),
                    visible: true,
                    from_table: posts,
                    to_table: users,
                    local_columns: vec![posts_user_id],
                    target_columns: vec![users_id],
                    cardinality: RelationCardinality::Singular,
                    nullable: false,
                    join_support: Some(posts_user_fk),
                    join_direction: Some(ForeignKeyDirection::Referencing),
                    supports: RelationSupports::default(),
                },
                Relation {
                    id: RelationId(1),
                    name: "posts".to_string(),
                    selector: "user_id".to_string(),
                    visible: true,
                    from_table: users,
                    to_table: posts,
                    local_columns: vec![users_id],
                    target_columns: vec![posts_user_id],
                    cardinality: RelationCardinality::Collection,
                    nullable: true,
                    join_support: Some(posts_user_fk),
                    join_direction: Some(ForeignKeyDirection::Referenced),
                    supports: RelationSupports::default(),
                },
            ],
            uniqueness_supports: Vec::new(),
        }
    }
}

fn btree_index(name: &str, column: ColumnId, is_unique: bool) -> Index {
    Index {
        name: Some(name.to_string()),
        access_method: "btree".to_string(),
        keys: vec![IndexKey {
            column,
            operator_class: None,
            capabilities: vec![IndexKeyCapability::Equality, IndexKeyCapability::Range],
            order: Some(IndexOrder {
                direction: IndexOrderDirection::Asc,
                nulls: IndexNullsPosition::Last,
            }),
        }],
        included_columns: Vec::new(),
        is_unique,
    }
}

fn searchable_index(name: &str, column: ColumnId) -> Index {
    Index {
        name: Some(name.to_string()),
        access_method: "gin".to_string(),
        keys: vec![IndexKey {
            column,
            operator_class: Some("public.gin_trgm_ops".to_string()),
            capabilities: vec![IndexKeyCapability::Like],
            order: None,
        }],
        included_columns: Vec::new(),
        is_unique: false,
    }
}
