//! Compiler-owned semantics for built-in logical types.
//!
//! Provider introspection derives effective operators, ordering, and text-cast
//! support. This table remains the single source for canonical wire, literal,
//! default, and aggregate behavior. Canonical DSQL names and PostgreSQL input
//! names are kept separately so policy declarations accept both without
//! widening [`DataType::from_database_type`].

use facet::Facet;
use std::collections::BTreeSet;

use super::{DataType, LiteralKind};
use crate::entities::aggregate::AggregateFunction;
use crate::entities::expression::ComparisonOp;

pub const MIN_SAFE_INTEGER: i64 = -9_007_199_254_740_991;
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Validation applied after a scalar's literal kind has matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum ScalarValidation {
    Any,
    Integer,
    SafeInteger,
    FiniteFloat,
}

/// How one scalar crosses the public JSON and PostgreSQL parameter boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
#[facet(rename_all = "snake_case")]
#[repr(u8)]
pub enum WireEncoding {
    Uuid,
    Text,
    Timestamptz,
    Integer,
    BigInteger,
    Numeric,
    Float,
    Boolean,
    Json,
    /// Bind a JSON string as PostgreSQL `text`, then cast to the provider type.
    TextCast,
    /// Selecting the value is allowed, but it cannot be used as an input.
    Unsupported,
}

impl WireEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uuid => "uuid",
            Self::Text => "text",
            Self::Timestamptz => "timestamptz",
            Self::Integer => "integer",
            Self::BigInteger => "big_integer",
            Self::Numeric => "numeric",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::Json => "json",
            Self::TextCast => "text_cast",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Accepted source kinds and value validation for one scalar surface.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct ScalarAcceptance {
    pub kinds: Vec<LiteralKind>,
    pub validation: ScalarValidation,
    pub description: String,
}

impl ScalarAcceptance {
    pub fn accepts(&self, kind: LiteralKind, value: &str) -> bool {
        if !self.kinds.contains(&kind) {
            return false;
        }
        match self.validation {
            ScalarValidation::Any => true,
            ScalarValidation::Integer => value.parse::<i64>().is_ok(),
            ScalarValidation::SafeInteger => value
                .parse::<i64>()
                .is_ok_and(|value| (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value)),
            ScalarValidation::FiniteFloat => value.parse::<f64>().is_ok_and(f64::is_finite),
        }
    }
}

/// Operand-dependent aggregate results supported by one catalog type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct AggregateCapabilities {
    pub minimum: bool,
    pub maximum: bool,
    pub sum: Option<DataType>,
    pub average: Option<DataType>,
}

impl AggregateCapabilities {
    pub fn result(&self, function: AggregateFunction, operand: DataType) -> Option<DataType> {
        match function {
            AggregateFunction::Min if self.minimum => Some(operand),
            AggregateFunction::Max if self.maximum => Some(operand),
            AggregateFunction::Sum => self.sum,
            AggregateFunction::Avg => self.average,
            AggregateFunction::Count | AggregateFunction::Exists => None,
            AggregateFunction::Min | AggregateFunction::Max => None,
        }
    }
}

/// Effective compiler semantics attached to one provider type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct TypeCapabilities {
    pub name: String,
    pub aliases: Vec<String>,
    /// Human-readable provider-ready description of the logical type.
    pub description: String,
    pub wire: WireEncoding,
    pub operators: Vec<ComparisonOp>,
    pub orderable: bool,
    pub literals: ScalarAcceptance,
    pub defaults: ScalarAcceptance,
    pub aggregates: AggregateCapabilities,
}

impl TypeCapabilities {
    pub fn builtin(data_type: DataType) -> Self {
        let definition = builtin_definition(data_type);
        Self {
            name: definition.name.to_string(),
            aliases: definition.aliases().map(ToString::to_string).collect(),
            description: definition.description.to_string(),
            wire: definition.wire,
            operators: definition.operators.to_vec(),
            orderable: definition.orderable,
            literals: ScalarAcceptance {
                kinds: definition.literal_kinds.to_vec(),
                validation: definition.literal_validation,
                description: definition.literal_description.to_string(),
            },
            defaults: ScalarAcceptance {
                kinds: definition.default_kinds.to_vec(),
                validation: definition.default_validation,
                description: definition.default_description.to_string(),
            },
            aggregates: definition.aggregates.clone(),
        }
    }

    pub fn supports(&self, operator: ComparisonOp) -> bool {
        self.operators.contains(&operator)
    }

    /// Builds effective comparison and ordering behavior from provider facts.
    pub(crate) fn provider(
        data_type: DataType,
        readable_type: &str,
        provider: &super::ProviderTypeFacts,
        operations: &BTreeSet<String>,
    ) -> Self {
        const PROVIDER_OPERATORS: [(&str, ComparisonOp); 7] = [
            ("=", ComparisonOp::Eq),
            ("<>", ComparisonOp::Ne),
            (">", ComparisonOp::Gt),
            (">=", ComparisonOp::Ge),
            ("<", ComparisonOp::Lt),
            ("<=", ComparisonOp::Le),
            ("~~", ComparisonOp::Like),
        ];
        let mut capabilities = Self::builtin(data_type);
        if data_type == DataType::Unknown && provider.supports_text_cast() {
            let text = Self::builtin(DataType::Text);
            capabilities.name = text.name;
            capabilities.aliases = text.aliases;
            capabilities.description = readable_type.to_string();
            capabilities.wire = WireEncoding::TextCast;
            capabilities.literals = text.literals;
            capabilities.defaults = text.defaults;
        }
        capabilities.operators = PROVIDER_OPERATORS
            .into_iter()
            .filter_map(|(written, operator)| operations.contains(written).then_some(operator))
            .collect();
        capabilities.orderable = provider.orderable;
        capabilities
    }
}

struct BuiltinTypeDefinition {
    data_type: DataType,
    name: &'static str,
    database_names: &'static [&'static str],
    description: &'static str,
    wire: WireEncoding,
    operators: &'static [ComparisonOp],
    orderable: bool,
    literal_kinds: &'static [LiteralKind],
    literal_validation: ScalarValidation,
    literal_description: &'static str,
    default_kinds: &'static [LiteralKind],
    default_validation: ScalarValidation,
    default_description: &'static str,
    aggregates: AggregateCapabilities,
}

impl BuiltinTypeDefinition {
    fn aliases(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.name).chain(
            self.database_names
                .iter()
                .copied()
                .filter(|alias| *alias != self.name),
        )
    }
}

const EQ: &[ComparisonOp] = &[ComparisonOp::Eq, ComparisonOp::Ne];
const ORDERED: &[ComparisonOp] = &[
    ComparisonOp::Eq,
    ComparisonOp::Ne,
    ComparisonOp::Gt,
    ComparisonOp::Ge,
    ComparisonOp::Lt,
    ComparisonOp::Le,
];
const TEXT: &[ComparisonOp] = &[ComparisonOp::Eq, ComparisonOp::Ne, ComparisonOp::Like];
const STRING: &[LiteralKind] = &[LiteralKind::String];
const NUMBER: &[LiteralKind] = &[LiteralKind::Number];
const BOOLEAN: &[LiteralKind] = &[LiteralKind::Boolean];
const NONE: &[LiteralKind] = &[];

const NO_AGGREGATES: AggregateCapabilities = AggregateCapabilities {
    minimum: false,
    maximum: false,
    sum: None,
    average: None,
};
const MIN_MAX: AggregateCapabilities = AggregateCapabilities {
    minimum: true,
    maximum: true,
    sum: None,
    average: None,
};

static BUILTIN_TYPES: [BuiltinTypeDefinition; 10] = [
    BuiltinTypeDefinition {
        data_type: DataType::Uuid,
        name: "uuid",
        database_names: &["uuid"],
        description: "UUID",
        wire: WireEncoding::Uuid,
        operators: EQ,
        orderable: true,
        literal_kinds: STRING,
        literal_validation: ScalarValidation::Any,
        literal_description: "string",
        default_kinds: STRING,
        default_validation: ScalarValidation::Any,
        default_description: "string",
        aggregates: NO_AGGREGATES,
    },
    BuiltinTypeDefinition {
        data_type: DataType::Text,
        name: "text",
        database_names: &["text", "varchar", "bpchar", "char", "name"],
        description: "text",
        wire: WireEncoding::Text,
        operators: TEXT,
        orderable: true,
        literal_kinds: STRING,
        literal_validation: ScalarValidation::Any,
        literal_description: "string",
        default_kinds: STRING,
        default_validation: ScalarValidation::Any,
        default_description: "string",
        aggregates: MIN_MAX,
    },
    BuiltinTypeDefinition {
        data_type: DataType::Timestamptz,
        name: "timestamptz",
        database_names: &["timestamptz", "timestamp with time zone"],
        description: "timestamp with time zone",
        wire: WireEncoding::Timestamptz,
        operators: ORDERED,
        orderable: true,
        literal_kinds: STRING,
        literal_validation: ScalarValidation::Any,
        literal_description: "string",
        default_kinds: STRING,
        default_validation: ScalarValidation::Any,
        default_description: "string",
        aggregates: MIN_MAX,
    },
    BuiltinTypeDefinition {
        data_type: DataType::Int,
        name: "int",
        database_names: &["int2", "int4", "integer", "smallint"],
        description: "integer",
        wire: WireEncoding::Integer,
        operators: ORDERED,
        orderable: true,
        literal_kinds: NUMBER,
        literal_validation: ScalarValidation::Integer,
        literal_description: "number",
        default_kinds: NUMBER,
        default_validation: ScalarValidation::SafeInteger,
        default_description: "safe integer",
        aggregates: AggregateCapabilities {
            minimum: true,
            maximum: true,
            sum: Some(DataType::Numeric),
            average: Some(DataType::Numeric),
        },
    },
    BuiltinTypeDefinition {
        data_type: DataType::BigInt,
        name: "bigint",
        database_names: &["int8", "bigint"],
        description: "64-bit integer",
        wire: WireEncoding::BigInteger,
        operators: ORDERED,
        orderable: true,
        literal_kinds: NUMBER,
        literal_validation: ScalarValidation::Integer,
        literal_description: "integer",
        default_kinds: STRING,
        default_validation: ScalarValidation::Integer,
        default_description: "signed 64-bit integer string",
        aggregates: AggregateCapabilities {
            minimum: true,
            maximum: true,
            sum: Some(DataType::Numeric),
            average: Some(DataType::Numeric),
        },
    },
    BuiltinTypeDefinition {
        data_type: DataType::Numeric,
        name: "numeric",
        database_names: &["numeric", "decimal"],
        description: "exact numeric",
        wire: WireEncoding::Numeric,
        operators: ORDERED,
        orderable: true,
        literal_kinds: NUMBER,
        literal_validation: ScalarValidation::Any,
        literal_description: "number",
        default_kinds: NUMBER,
        default_validation: ScalarValidation::Any,
        default_description: "number",
        aggregates: AggregateCapabilities {
            minimum: false,
            maximum: false,
            sum: Some(DataType::Numeric),
            average: Some(DataType::Numeric),
        },
    },
    BuiltinTypeDefinition {
        data_type: DataType::Float,
        name: "float",
        database_names: &["float4", "real", "float8", "double precision"],
        description: "floating-point number",
        wire: WireEncoding::Float,
        operators: ORDERED,
        orderable: true,
        literal_kinds: NUMBER,
        literal_validation: ScalarValidation::Any,
        literal_description: "number",
        default_kinds: NUMBER,
        default_validation: ScalarValidation::FiniteFloat,
        default_description: "finite number",
        aggregates: AggregateCapabilities {
            minimum: false,
            maximum: false,
            sum: Some(DataType::Float),
            average: Some(DataType::Float),
        },
    },
    BuiltinTypeDefinition {
        data_type: DataType::Boolean,
        name: "boolean",
        database_names: &["bool", "boolean"],
        description: "boolean",
        wire: WireEncoding::Boolean,
        operators: EQ,
        orderable: true,
        literal_kinds: BOOLEAN,
        literal_validation: ScalarValidation::Any,
        literal_description: "boolean",
        default_kinds: BOOLEAN,
        default_validation: ScalarValidation::Any,
        default_description: "boolean",
        aggregates: NO_AGGREGATES,
    },
    BuiltinTypeDefinition {
        data_type: DataType::Json,
        name: "json",
        database_names: &["json", "jsonb"],
        description: "JSON",
        wire: WireEncoding::Json,
        operators: EQ,
        orderable: true,
        literal_kinds: STRING,
        literal_validation: ScalarValidation::Any,
        literal_description: "string",
        default_kinds: NONE,
        default_validation: ScalarValidation::Any,
        default_description: "value",
        aggregates: NO_AGGREGATES,
    },
    BuiltinTypeDefinition {
        data_type: DataType::Unknown,
        name: "unknown",
        database_names: &[],
        description: "unmapped provider type",
        wire: WireEncoding::Unsupported,
        operators: EQ,
        orderable: true,
        literal_kinds: NONE,
        literal_validation: ScalarValidation::Any,
        literal_description: "value",
        default_kinds: NONE,
        default_validation: ScalarValidation::Any,
        default_description: "value",
        aggregates: NO_AGGREGATES,
    },
];

fn builtin_definition(data_type: DataType) -> &'static BuiltinTypeDefinition {
    match data_type {
        DataType::Uuid => &BUILTIN_TYPES[0],
        DataType::Text => &BUILTIN_TYPES[1],
        DataType::Timestamptz => &BUILTIN_TYPES[2],
        DataType::Int => &BUILTIN_TYPES[3],
        DataType::BigInt => &BUILTIN_TYPES[4],
        DataType::Numeric => &BUILTIN_TYPES[5],
        DataType::Float => &BUILTIN_TYPES[6],
        DataType::Boolean => &BUILTIN_TYPES[7],
        DataType::Json => &BUILTIN_TYPES[8],
        DataType::Unknown => &BUILTIN_TYPES[9],
    }
}

pub(super) fn data_type_name(data_type: DataType) -> &'static str {
    builtin_definition(data_type).name
}

pub(super) fn data_type_from_database_name(name: &str) -> DataType {
    BUILTIN_TYPES
        .iter()
        .find(|definition| definition.database_names.contains(&name))
        .map_or(DataType::Unknown, |definition| definition.data_type)
}

pub(super) fn data_type_from_logical_name(name: &str) -> Option<DataType> {
    BUILTIN_TYPES
        .iter()
        .find(|definition| definition.aliases().any(|alias| alias == name))
        .map(|definition| definition.data_type)
}
