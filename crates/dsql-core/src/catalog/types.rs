use super::{ColumnId, ColumnKey, ForeignKeyId, SchemaId, SchemaKey, TableId, TableKey};
use crate::entities::expression::ComparisonOp;
use facet::Facet;

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Catalog {
    pub default_schema: String,
    pub schemas: Vec<Schema>,
    pub tables: Vec<Table>,
    pub columns: Vec<Column>,
    pub foreign_keys: Vec<ForeignKey>,
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
    pub unique_constraints: Vec<Vec<ColumnId>>,
    pub indexes: Vec<Index>,
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
    pub name: Option<String>,
    pub from_columns: Vec<ColumnId>,
    pub to_columns: Vec<ColumnId>,
    pub from_table: TableId,
    pub to_table: TableId,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Index {
    pub name: Option<String>,
    pub columns: Vec<ColumnId>,
    pub is_unique: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum DataType {
    Uuid,
    Text,
    Timestamptz,
    Int,
    Boolean,
    Json,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(u8)]
pub enum LiteralKind {
    String,
    Number,
    Boolean,
    Null,
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
        candidates: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationCardinality {
    Collection,
    Singular,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationField<'a> {
    pub name: &'a str,
    pub selector: String,
    pub table: &'a Table,
    pub foreign_key: &'a ForeignKey,
}

impl Catalog {
    pub const DEFAULT_SCHEMA: &'static str = "public";

    pub fn with_default_schema(mut self, default_schema: impl Into<String>) -> Self {
        self.default_schema = default_schema.into();
        self
    }

    pub fn default_schema(&self) -> &str {
        &self.default_schema
    }
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
        unique_constraints: Vec<Vec<ColumnId>>,
        indexes: Vec<Index>,
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
            unique_constraints,
            indexes,
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
            Self::Unknown => "unknown",
        }
    }

    pub fn from_database_type(database_type: &str) -> Self {
        match database_type {
            "bool" | "boolean" => Self::Boolean,
            "int2" | "int4" | "int8" | "integer" | "smallint" | "bigint" => Self::Int,
            "json" | "jsonb" => Self::Json,
            "text" | "varchar" | "bpchar" | "char" | "name" => Self::Text,
            "timestamptz" | "timestamp with time zone" => Self::Timestamptz,
            "uuid" => Self::Uuid,
            _ => Self::Unknown,
        }
    }

    pub fn operator_ops(self) -> &'static [ComparisonOp] {
        match self {
            Self::Int | Self::Timestamptz => &[
                ComparisonOp::Eq,
                ComparisonOp::Ne,
                ComparisonOp::Gt,
                ComparisonOp::Ge,
                ComparisonOp::Lt,
                ComparisonOp::Le,
            ],
            Self::Text => &[ComparisonOp::Eq, ComparisonOp::Ne, ComparisonOp::Like],
            Self::Uuid | Self::Boolean | Self::Json | Self::Unknown => {
                &[ComparisonOp::Eq, ComparisonOp::Ne]
            }
        }
    }

    pub fn accepts_literal_kind(self, literal: LiteralKind) -> bool {
        match self {
            Self::Int => literal == LiteralKind::Number,
            Self::Boolean => literal == LiteralKind::Boolean,
            Self::Text | Self::Uuid | Self::Timestamptz | Self::Json => {
                literal == LiteralKind::String
            }
            Self::Unknown => true,
        }
    }

    pub fn accepts_literal_value(self, literal: LiteralKind, value: &str) -> bool {
        if !self.accepts_literal_kind(literal) {
            return false;
        }
        match (self, literal) {
            (Self::Int, LiteralKind::Number) => value.parse::<i64>().is_ok(),
            _ => true,
        }
    }

    pub fn expected_literal_description(self) -> &'static str {
        match self {
            Self::Int => "number",
            Self::Boolean => "boolean",
            Self::Text | Self::Uuid | Self::Timestamptz | Self::Json => "string",
            Self::Unknown => "value",
        }
    }
}

impl LiteralKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Null => "null",
        }
    }
}

/// A `schema::name` table reference as written in source, borrowed from
/// the fact that carries it.
#[derive(Clone, Copy, Debug)]
pub struct TableRef<'a> {
    pub schema: Option<&'a str>,
    pub name: &'a str,
}

impl<'a> TableRef<'a> {
    /// Splits a raw qualified-name text (`schema::name` or `name`).
    pub fn parse(raw: &'a str) -> Self {
        match raw.split_once("::") {
            Some((schema, name)) => Self {
                schema: Some(schema),
                name,
            },
            None => Self {
                schema: None,
                name: raw,
            },
        }
    }

    pub fn display_text(&self) -> String {
        match self.schema {
            Some(schema) => format!("{schema}::{}", self.name),
            None => self.name.to_string(),
        }
    }
}

/// A field reference inside a selection: `schema::name->selector` with the
/// schema and selector optional.
#[derive(Clone, Copy, Debug)]
pub struct FieldRef<'a> {
    pub target: TableRef<'a>,
    pub selector: Option<&'a str>,
}

impl FieldRef<'_> {
    pub fn display_text(&self) -> String {
        match self.selector {
            Some(selector) => format!("{}->{selector}", self.target.display_text()),
            None => self.target.display_text(),
        }
    }
}
