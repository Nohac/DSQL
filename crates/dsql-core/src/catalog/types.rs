use super::{
    ColumnId, ColumnKey, ForeignKeyId, ObjectType, RelationId, SchemaId, SchemaKey, TableId,
    TableKey,
};
use crate::entities::expression::ComparisonOp;
use facet::Facet;
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Catalog {
    pub default_schema: String,
    pub schemas: Vec<Schema>,
    pub tables: Vec<Table>,
    pub columns: Vec<Column>,
    pub foreign_keys: Vec<ForeignKey>,
    /// Directional relationships exposed by the effective catalog.
    pub relations: Vec<Relation>,
    /// Provider and overlay proofs for effective uniqueness facts.
    pub uniqueness_supports: Vec<UniquenessSupport>,
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
    /// Provider object kind retained for overlay validation.
    pub object_type: ObjectType,
    /// Whether query-facing catalog lookup exposes this object.
    pub visible: bool,
    pub description: Option<String>,
    /// Source that declares this object.
    pub declaration: Option<CatalogSupport>,
    /// Source that owns the effective description.
    pub description_support: Option<CatalogSupport>,
    /// Sources that changed query-facing exposure.
    pub exposure_support: Vec<CatalogSupport>,
    pub columns: Vec<ColumnId>,
    pub primary_key: Vec<ColumnId>,
    pub unique_constraints: Vec<Vec<ColumnId>>,
    pub indexes: Vec<Index>,
    /// Directional effective relationships originating at this object.
    pub relations: Vec<RelationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Column {
    pub id: ColumnId,
    pub key: ColumnKey,
    pub table: TableId,
    pub name: String,
    pub description: Option<String>,
    /// Whether query-facing catalog lookup exposes this column.
    pub visible: bool,
    /// Source that declares this column.
    pub declaration: Option<CatalogSupport>,
    /// Source that owns the effective description.
    pub description_support: Option<CatalogSupport>,
    /// Sources that changed query-facing exposure.
    pub exposure_support: Vec<CatalogSupport>,
    pub database_type: String,
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

/// One directional relationship exposed in the effective catalog.
///
/// Provider foreign keys remain proof facts; query planning consumes this
/// oriented mapping so authored relationships never need fake constraints.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct Relation {
    /// Dense identity within the effective catalog.
    pub id: RelationId,
    /// Query-facing field name.
    pub name: String,
    /// Explicit selector used to disambiguate provider relationships.
    pub selector: String,
    /// Whether query-facing catalog lookup exposes this direction.
    pub visible: bool,
    /// Object on which the relationship field is selected.
    pub from_table: TableId,
    /// Object produced by traversing the relationship.
    pub to_table: TableId,
    /// Ordered join columns on [`Relation::from_table`].
    pub local_columns: Vec<ColumnId>,
    /// Ordered join columns on [`Relation::to_table`].
    pub target_columns: Vec<ColumnId>,
    /// Result cardinality inferred from effective uniqueness proofs.
    pub cardinality: RelationCardinality,
    /// Whether a singular traversal may produce no related row.
    pub nullable: bool,
    /// Matching provider foreign key, when one proves the authored join.
    pub join_support: Option<ForeignKeyId>,
    /// Direction of [`Relation::join_support`] relative to this relationship.
    pub join_direction: Option<ForeignKeyDirection>,
    /// Independent provenance for declaration and derived proof classes.
    pub supports: RelationSupports,
}

/// Direction in which a relationship traverses its supporting foreign key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(u8)]
pub enum ForeignKeyDirection {
    /// From the object containing the foreign-key columns to its target.
    Referencing,
    /// From the referenced object back to rows containing the foreign key.
    Referenced,
}

/// Origin kind for one effective catalog fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum CatalogSupportKind {
    /// Generated metadata supplied by the database provider.
    Provider,
    /// Authored metadata supplied by a project overlay.
    Overlay,
}

/// Optional source range for provenance-aware catalog navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct CatalogSourceRange {
    /// Inclusive byte offset in the support source.
    pub start: usize,
    /// Exclusive byte offset in the support source.
    pub end: usize,
}

/// Stable source identity for one provider or overlay fact.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct CatalogSupport {
    /// Whether the fact came from generated provider metadata or an overlay.
    pub kind: CatalogSupportKind,
    /// Stable source path used for diagnostics and navigation.
    pub path: String,
    /// Stable semantic path to the supported item within the source.
    pub item_path: String,
    /// Exact source range when the decoder can provide one.
    pub range: Option<CatalogSourceRange>,
}

/// Distinct proof classes retained for one effective relationship.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Facet)]
pub struct RelationSupports {
    /// Source that owns the exposed relationship declaration and name.
    pub declaration: Option<CatalogSupport>,
    /// Sources proving the ordered join mapping.
    pub join: Vec<CatalogSupport>,
    /// Sources proving at-most-one target row.
    pub cardinality: Vec<CatalogSupport>,
    /// Sources proving that a singular target row must exist.
    pub presence: Vec<CatalogSupport>,
    /// Sources changing whether the relationship is exposed.
    pub exposure: Vec<CatalogSupport>,
}

/// Named provider or overlay evidence that one ordered column tuple is unique.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct UniquenessSupport {
    /// Object whose ordered column tuple is unique.
    pub table: TableId,
    /// Stable provider or authored proof name.
    pub name: String,
    /// Ordered unique column tuple.
    pub columns: Vec<ColumnId>,
    /// Source that supplies the uniqueness proof.
    pub support: CatalogSupport,
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
    Numeric,
    Float,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(u8)]
pub enum RelationCardinality {
    Collection,
    Singular,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationField<'a> {
    pub name: &'a str,
    pub selector: String,
    pub table: &'a Table,
    pub relation: &'a Relation,
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

    /// Computes one deterministic fingerprint over effective catalog
    /// semantics. Dense IDs and provenance byte ranges are deliberately
    /// excluded; semantic keys, visibility, proofs, and source identities
    /// participate.
    pub fn semantic_fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.default_schema.hash(&mut hasher);
        let mut tables = self.tables.iter().collect::<Vec<_>>();
        tables.sort_by(|left, right| {
            left.key
                .schema
                .cmp(&right.key.schema)
                .then_with(|| left.key.table.cmp(&right.key.table))
        });
        for table in tables {
            table.key.hash(&mut hasher);
            table.object_type.hash(&mut hasher);
            table.visible.hash(&mut hasher);
            table.description.hash(&mut hasher);
            hash_support(table.declaration.as_ref(), &mut hasher);
            hash_support(table.description_support.as_ref(), &mut hasher);
            hash_supports(&table.exposure_support, &mut hasher);
            hash_column_tuple(self, &table.primary_key, &mut hasher);
            let mut unique_constraints = table.unique_constraints.iter().collect::<Vec<_>>();
            unique_constraints.sort_by(|left, right| {
                column_tuple_key(self, left).cmp(&column_tuple_key(self, right))
            });
            unique_constraints.len().hash(&mut hasher);
            for constraint in unique_constraints {
                hash_column_tuple(self, constraint, &mut hasher);
            }
            let mut indexes = table.indexes.iter().collect::<Vec<_>>();
            indexes.sort_by(|left, right| {
                (
                    &left.name,
                    left.is_unique,
                    column_tuple_key(self, &left.columns),
                )
                    .cmp(&(
                        &right.name,
                        right.is_unique,
                        column_tuple_key(self, &right.columns),
                    ))
            });
            indexes.len().hash(&mut hasher);
            for index in indexes {
                index.name.hash(&mut hasher);
                index.is_unique.hash(&mut hasher);
                hash_column_tuple(self, &index.columns, &mut hasher);
            }
        }
        let mut columns = self.columns.iter().collect::<Vec<_>>();
        columns.sort_by(|left, right| {
            left.key.schema.cmp(&right.key.schema).then_with(|| {
                left.key
                    .table
                    .cmp(&right.key.table)
                    .then_with(|| left.key.column.cmp(&right.key.column))
            })
        });
        for column in columns {
            column.key.hash(&mut hasher);
            column.description.hash(&mut hasher);
            column.database_type.hash(&mut hasher);
            column.data_type.hash(&mut hasher);
            column.not_null.hash(&mut hasher);
            column.is_unique.hash(&mut hasher);
            column.is_indexed.hash(&mut hasher);
            column.visible.hash(&mut hasher);
            hash_support(column.declaration.as_ref(), &mut hasher);
            hash_support(column.description_support.as_ref(), &mut hasher);
            hash_supports(&column.exposure_support, &mut hasher);
        }
        let mut relations = self.relations.iter().collect::<Vec<_>>();
        relations.sort_by(|left, right| {
            relation_key(self, left)
                .cmp(&relation_key(self, right))
                .then_with(|| {
                    column_tuple_key(self, &left.local_columns)
                        .cmp(&column_tuple_key(self, &right.local_columns))
                })
                .then_with(|| {
                    column_tuple_key(self, &left.target_columns)
                        .cmp(&column_tuple_key(self, &right.target_columns))
                })
                .then_with(|| {
                    foreign_key_direction_key(left.join_direction)
                        .cmp(&foreign_key_direction_key(right.join_direction))
                })
        });
        for relation in relations {
            relation_key(self, relation).hash(&mut hasher);
            relation.visible.hash(&mut hasher);
            relation.cardinality.hash(&mut hasher);
            relation.nullable.hash(&mut hasher);
            relation.join_direction.hash(&mut hasher);
            hash_column_tuple(self, &relation.local_columns, &mut hasher);
            hash_column_tuple(self, &relation.target_columns, &mut hasher);
            hash_support(relation.supports.declaration.as_ref(), &mut hasher);
            hash_supports(&relation.supports.join, &mut hasher);
            hash_supports(&relation.supports.cardinality, &mut hasher);
            hash_supports(&relation.supports.presence, &mut hasher);
            hash_supports(&relation.supports.exposure, &mut hasher);
        }
        let mut uniqueness = self.uniqueness_supports.iter().collect::<Vec<_>>();
        uniqueness.sort_by(|left, right| {
            let left_table = &self.tables[left.table.0].key;
            let right_table = &self.tables[right.table.0].key;
            (left_table, &left.name).cmp(&(right_table, &right.name))
        });
        for proof in uniqueness {
            self.tables[proof.table.0].key.hash(&mut hasher);
            proof.name.hash(&mut hasher);
            hash_column_tuple(self, &proof.columns, &mut hasher);
            hash_support(Some(&proof.support), &mut hasher);
        }
        hasher.finish()
    }
}

fn relation_key<'a>(
    catalog: &'a Catalog,
    relation: &'a Relation,
) -> (&'a TableKey, &'a str, &'a str, &'a TableKey) {
    (
        &catalog.tables[relation.from_table.0].key,
        relation.name.as_str(),
        relation.selector.as_str(),
        &catalog.tables[relation.to_table.0].key,
    )
}

fn hash_column_tuple(catalog: &Catalog, columns: &[ColumnId], hasher: &mut DefaultHasher) {
    columns.len().hash(hasher);
    for column in columns {
        catalog.columns[column.0].key.hash(hasher);
    }
}

fn column_tuple_key<'a>(catalog: &'a Catalog, columns: &[ColumnId]) -> Vec<&'a ColumnKey> {
    columns
        .iter()
        .map(|column| &catalog.columns[column.0].key)
        .collect()
}

fn hash_support(support: Option<&CatalogSupport>, hasher: &mut DefaultHasher) {
    if let Some(support) = support {
        support.kind.hash(hasher);
        support.path.hash(hasher);
        support.item_path.hash(hasher);
    }
}

fn hash_supports(supports: &[CatalogSupport], hasher: &mut DefaultHasher) {
    let mut supports = supports.iter().collect::<Vec<_>>();
    supports.sort_by_key(|support| support_key(support));
    supports.len().hash(hasher);
    for support in supports {
        hash_support(Some(support), hasher);
    }
}

fn support_key(support: &CatalogSupport) -> (u8, &str, &str) {
    (
        match support.kind {
            CatalogSupportKind::Provider => 0,
            CatalogSupportKind::Overlay => 1,
        },
        support.path.as_str(),
        support.item_path.as_str(),
    )
}

fn foreign_key_direction_key(direction: Option<ForeignKeyDirection>) -> u8 {
    match direction {
        None => 0,
        Some(ForeignKeyDirection::Referencing) => 1,
        Some(ForeignKeyDirection::Referenced) => 2,
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
    #[expect(
        clippy::too_many_arguments,
        reason = "catalog builders provide one value for each table fact"
    )]
    pub fn new(
        id: TableId,
        schema_id: SchemaId,
        schema: &str,
        name: &str,
        object_type: ObjectType,
        description: Option<String>,
        columns: Vec<ColumnId>,
        primary_key: Vec<ColumnId>,
        unique_constraints: Vec<Vec<ColumnId>>,
        indexes: Vec<Index>,
        relations: Vec<RelationId>,
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
            object_type,
            visible: true,
            description,
            declaration: None,
            description_support: None,
            exposure_support: Vec::new(),
            columns,
            primary_key,
            unique_constraints,
            indexes,
            relations,
        }
    }
}

impl Column {
    #[expect(
        clippy::too_many_arguments,
        reason = "catalog builders provide one value for each column fact"
    )]
    pub fn new(
        id: ColumnId,
        table: TableId,
        schema: &str,
        table_name: &str,
        name: &str,
        description: Option<String>,
        database_type: &str,
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
            description,
            visible: true,
            declaration: None,
            description_support: None,
            exposure_support: Vec::new(),
            database_type: database_type.to_string(),
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
            Self::Numeric => "numeric",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::Json => "json",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_database_type(database_type: &str) -> Self {
        match database_type {
            "bool" | "boolean" => Self::Boolean,
            "int2" | "int4" | "int8" | "integer" | "smallint" | "bigint" => Self::Int,
            "numeric" | "decimal" => Self::Numeric,
            "float4" | "real" | "float8" | "double precision" => Self::Float,
            "json" | "jsonb" => Self::Json,
            "text" | "varchar" | "bpchar" | "char" | "name" => Self::Text,
            "timestamptz" | "timestamp with time zone" => Self::Timestamptz,
            "uuid" => Self::Uuid,
            _ => Self::Unknown,
        }
    }

    pub fn operator_ops(self) -> &'static [ComparisonOp] {
        match self {
            Self::Int | Self::Numeric | Self::Float | Self::Timestamptz => &[
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
            Self::Int | Self::Numeric | Self::Float => literal == LiteralKind::Number,
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
            Self::Int | Self::Numeric | Self::Float => "number",
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
