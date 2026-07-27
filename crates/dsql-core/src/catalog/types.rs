use super::{
    ColumnId, ColumnKey, ForeignKeyId, ObjectType, ProviderTypeFacts, RelationId, SchemaId,
    SchemaKey, TableId, TableKey, TypeCapabilities, TypeId, TypeKey,
};
use facet::Facet;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct Catalog {
    pub default_schema: String,
    pub schemas: Vec<Schema>,
    pub tables: Vec<Table>,
    /// Dense arena of effective provider types.
    pub types: Vec<CatalogType>,
    /// Fast lookup for stable provider identities; reconstructed by catalog loading.
    #[facet(skip)]
    pub(crate) type_ids: HashMap<TypeKey, TypeId>,
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

/// One provider type resolved into the effective catalog.
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct CatalogType {
    /// Dense identity within [`Catalog::types`].
    pub id: TypeId,
    /// Stable schema-qualified provider identity.
    pub key: TypeKey,
    /// Public logical type consumed by language semantics.
    pub data_type: DataType,
    /// Resolved provider structure. Non-scalars keep [`DataType::Unknown`] at
    /// the outer type and expose their logical leaf through this graph.
    pub shape: CatalogTypeShape,
    /// Provider-formatted spelling for this type without column modifiers.
    pub readable_type: String,
    /// Native classification facts, absent for synthetic compiler fixtures.
    pub provider: Option<ProviderTypeFacts>,
    /// Query-facing behavior supplied by the compiler or provider.
    pub capabilities: TypeCapabilities,
}

/// Resolved structural relationship within [`Catalog::types`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[repr(u8)]
pub enum CatalogTypeShape {
    Scalar,
    Domain { base: TypeId },
    Array { element: TypeId },
}

/// Effective public value shape after domain wrappers are resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogValueShape<'a> {
    /// One scalar value with the effective provider leaf type.
    Scalar { leaf: &'a CatalogType },
    /// One PostgreSQL array whose elements use the effective provider leaf type.
    DatabaseArray { element: &'a CatalogType },
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
    /// Type resolved through [`Catalog::types`].
    pub type_id: TypeId,
    /// Provider display including modifiers and qualification; falls back to
    /// the logical type name when metadata carries no provider display.
    pub formatted_type: String,
    /// Raw PostgreSQL type modifier, when supplied by introspection.
    pub type_modifier: Option<i32>,
    pub not_null: bool,
    pub is_unique: bool,
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
    pub access_method: String,
    pub keys: Vec<IndexKey>,
    pub included_columns: Vec<ColumnId>,
    pub is_unique: bool,
}

/// One ordered key participating in a physical database index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct IndexKey {
    pub column: ColumnId,
    pub operator_class: Option<String>,
    pub capabilities: Vec<IndexKeyCapability>,
    pub order: Option<IndexOrder>,
}

/// Provider-neutral operation class supported by an [`IndexKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum IndexKeyCapability {
    Equality,
    Range,
    Like,
}

/// Physical ordering retained for an orderable [`IndexKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
pub struct IndexOrder {
    pub direction: IndexOrderDirection,
    pub nulls: IndexNullsPosition,
}

/// Physical direction of one orderable index key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum IndexOrderDirection {
    Asc,
    Desc,
}

/// Physical null placement of one orderable index key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum IndexNullsPosition {
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum DataType {
    Uuid,
    Text,
    Timestamptz,
    Int,
    #[facet(rename = "bigint")]
    BigInt,
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

    /// Builds an empty catalog for editor fallback contexts.
    pub fn empty() -> Self {
        Self {
            default_schema: Self::DEFAULT_SCHEMA.to_string(),
            schemas: Vec::new(),
            tables: Vec::new(),
            types: Vec::new(),
            type_ids: HashMap::new(),
            columns: Vec::new(),
            foreign_keys: Vec::new(),
            relations: Vec::new(),
            uniqueness_supports: Vec::new(),
        }
    }

    pub fn with_default_schema(mut self, default_schema: impl Into<String>) -> Self {
        self.default_schema = default_schema.into();
        self
    }

    pub fn default_schema(&self) -> &str {
        &self.default_schema
    }

    /// Resolves domains and array elements into one public value shape.
    pub fn value_shape_for_type(&self, type_id: TypeId) -> Option<CatalogValueShape<'_>> {
        let mut current = self.types.get(type_id.0)?;
        loop {
            match current.shape {
                CatalogTypeShape::Scalar => {
                    return Some(CatalogValueShape::Scalar { leaf: current });
                }
                CatalogTypeShape::Domain { base } => {
                    current = self.types.get(base.0)?;
                }
                CatalogTypeShape::Array { element } => {
                    let mut element = self.types.get(element.0)?;
                    loop {
                        match element.shape {
                            CatalogTypeShape::Scalar => {
                                return Some(CatalogValueShape::DatabaseArray { element });
                            }
                            CatalogTypeShape::Domain { base } => {
                                element = self.types.get(base.0)?;
                            }
                            CatalogTypeShape::Array { element: nested } => {
                                // Arrays of domains whose base is itself an
                                // array deliberately collapse to the terminal
                                // scalar. DsqlDatabaseArray<T> represents the
                                // resulting arbitrary JSON nesting depth.
                                element = self.types.get(nested.0)?;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Resolves the public value shape for one catalog column.
    pub fn value_shape_for_column(&self, column: ColumnId) -> Option<CatalogValueShape<'_>> {
        self.columns
            .get(column.0)
            .and_then(|column| self.value_shape_for_type(column.type_id))
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
                    &left.access_method,
                    left.is_unique,
                    index_key(self, left),
                )
                    .cmp(&(
                        &right.name,
                        &right.access_method,
                        right.is_unique,
                        index_key(self, right),
                    ))
            });
            indexes.len().hash(&mut hasher);
            for index in indexes {
                index.name.hash(&mut hasher);
                index.access_method.hash(&mut hasher);
                index.is_unique.hash(&mut hasher);
                index.keys.len().hash(&mut hasher);
                for key in &index.keys {
                    self.columns[key.column.0].key.hash(&mut hasher);
                    key.operator_class.hash(&mut hasher);
                    key.capabilities.hash(&mut hasher);
                    key.order.hash(&mut hasher);
                }
                hash_column_tuple(self, &index.included_columns, &mut hasher);
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
        let mut referenced_type_set = HashSet::new();
        let mut pending_types = columns
            .iter()
            .map(|column| column.type_id)
            .collect::<Vec<_>>();
        while let Some(type_id) = pending_types.pop() {
            if !referenced_type_set.insert(type_id) {
                continue;
            }
            match self.types[type_id.0].shape {
                CatalogTypeShape::Scalar => {}
                CatalogTypeShape::Domain { base } => pending_types.push(base),
                CatalogTypeShape::Array { element } => pending_types.push(element),
            }
        }
        let mut referenced_types = referenced_type_set.into_iter().collect::<Vec<_>>();
        referenced_types
            .sort_by(|left, right| self.types[left.0].key.cmp(&self.types[right.0].key));
        for column in columns {
            column.key.hash(&mut hasher);
            column.description.hash(&mut hasher);
            let data_type = &self.types[column.type_id.0];
            data_type.key.hash(&mut hasher);
            column.formatted_type.hash(&mut hasher);
            column.type_modifier.hash(&mut hasher);
            column.not_null.hash(&mut hasher);
            column.is_unique.hash(&mut hasher);
            column.visible.hash(&mut hasher);
            hash_support(column.declaration.as_ref(), &mut hasher);
            hash_support(column.description_support.as_ref(), &mut hasher);
            hash_supports(&column.exposure_support, &mut hasher);
        }
        for type_id in referenced_types {
            let data_type = &self.types[type_id.0];
            data_type.key.hash(&mut hasher);
            data_type.data_type.hash(&mut hasher);
            match data_type.shape {
                CatalogTypeShape::Scalar => 0_u8.hash(&mut hasher),
                CatalogTypeShape::Domain { base } => {
                    1_u8.hash(&mut hasher);
                    self.types[base.0].key.hash(&mut hasher);
                }
                CatalogTypeShape::Array { element } => {
                    2_u8.hash(&mut hasher);
                    self.types[element.0].key.hash(&mut hasher);
                }
            }
            data_type.readable_type.hash(&mut hasher);
            data_type.provider.hash(&mut hasher);
            data_type.capabilities.hash(&mut hasher);
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

type IndexKeySortKey = (
    ColumnKey,
    Option<String>,
    Vec<IndexKeyCapability>,
    Option<IndexOrder>,
);
type IndexSortKey = (Vec<IndexKeySortKey>, Vec<ColumnKey>);

fn index_key(catalog: &Catalog, index: &Index) -> IndexSortKey {
    (
        index
            .keys
            .iter()
            .map(|key| {
                (
                    catalog.columns[key.column.0].key.clone(),
                    key.operator_class.clone(),
                    key.capabilities.clone(),
                    key.order,
                )
            })
            .collect(),
        index
            .included_columns
            .iter()
            .map(|column| catalog.columns[column.0].key.clone())
            .collect(),
    )
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
        type_id: TypeId,
        formatted_type: String,
        type_modifier: Option<i32>,
        not_null: bool,
        is_unique: bool,
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
            type_id,
            formatted_type,
            type_modifier,
            not_null,
            is_unique,
        }
    }
}

impl CatalogType {
    /// Logical scalar category exposed to language consumers.
    ///
    /// Provider values carried through the text-cast wire behave as text in
    /// literals and host contracts while [`CatalogType::key`] retains the
    /// exact cast target.
    pub fn logical_data_type(&self) -> DataType {
        if self.capabilities.wire == super::WireEncoding::TextCast {
            DataType::Text
        } else {
            self.data_type
        }
    }

    /// Constructs a provider-less type using the compiler-owned capability table.
    pub fn builtin(id: TypeId, key: TypeKey, data_type: DataType) -> Self {
        let readable_type = key.name.clone();
        Self {
            id,
            key,
            data_type,
            shape: CatalogTypeShape::Scalar,
            readable_type,
            provider: None,
            capabilities: TypeCapabilities::builtin(data_type),
        }
    }
}

impl DataType {
    pub const ALL: [Self; 10] = [
        Self::Uuid,
        Self::Text,
        Self::Timestamptz,
        Self::Int,
        Self::BigInt,
        Self::Numeric,
        Self::Float,
        Self::Boolean,
        Self::Json,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        super::capabilities::data_type_name(self)
    }

    pub fn from_database_type(database_type: &str) -> Self {
        super::capabilities::data_type_from_database_name(database_type)
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
