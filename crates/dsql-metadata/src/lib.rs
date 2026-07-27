use facet::Facet;

#[cfg(feature = "render")]
const BUILD_MANIFEST_SCHEMA_ID: &str = "https://dsql.dev/schemas/build-manifest.schema.json";

/// Manifest format version 5: result fields carry an explicit value shape.
pub const BUILD_MANIFEST_VERSION: u32 = 5;
#[cfg(feature = "render")]
const JSON_SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

#[cfg(feature = "render")]
#[derive(Debug, Facet)]
#[facet(skip_all_unless_truthy)]
struct BuildManifestJsonSchema {
    #[facet(rename = "$id")]
    id: &'static str,
    #[facet(flatten)]
    schema: facet_json_schema::JsonSchema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet, strum::AsRefStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum DefinitionKind {
    Query,
    Fragment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet, strum::AsRefStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum SqlDialect {
    Postgres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet, strum::AsRefStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum ResultFieldKind {
    Scalar,
    Object,
    Array,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet, strum::AsRefStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum ResultValueShape {
    Scalar,
    DatabaseArray,
    Object,
}

/// How one scalar crosses the public JSON and PostgreSQL parameter boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet, strum::AsRefStr, strum::IntoStaticStr)]
#[facet(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
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
        self.into()
    }
}

#[derive(Clone, Debug, Facet)]
pub struct BuildManifest {
    pub version: u32,
    /// The project-monotonic generation this manifest commits
    /// (docs/spec/build-daemon.md).
    #[facet(rename = "generationId")]
    pub generation_id: u64,
    pub operations: Vec<OperationManifestEntry>,
    pub fragments: Vec<FragmentManifestEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct OperationManifestEntry {
    pub name: String,
    pub kind: DefinitionKind,
    pub path: String,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct FragmentManifestEntry {
    pub name: String,
    pub kind: DefinitionKind,
    pub path: String,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug, Facet)]
pub struct OperationMetadata {
    pub name: String,
    pub kind: DefinitionKind,
    pub sql: SqlMetadata,
    pub result: ResultShape,
    pub params: Vec<InputField>,
    pub input: Vec<InputField>,
    pub context: Vec<InputField>,
    pub dynamic_inputs: Vec<DynamicInputMetadata>,
    pub policies: Vec<PolicyMetadata>,
    pub handoffs: Vec<HandoffMetadata>,
    pub fragment_spreads: Vec<FragmentSpreadMetadata>,
    pub source_map: Vec<SourceMapEntry>,
}

#[derive(Clone, Debug, Facet)]
pub struct FragmentMetadata {
    pub name: String,
    pub kind: DefinitionKind,
    pub table: String,
    pub result: ResultShape,
    pub params: Vec<InputField>,
    pub input: Vec<InputField>,
    pub dynamic_inputs: Vec<DynamicInputMetadata>,
    /// Fragment spreads the body expanded, path-qualified like the
    /// operation-level field (the empty path is the fragment root):
    /// renderers compose fragment types by reuse from this.
    pub fragment_spreads: Vec<FragmentSpreadMetadata>,
    pub source_map: Vec<SourceMapEntry>,
}

#[derive(Clone, Debug, Facet)]
pub struct FragmentSpreadMetadata {
    pub path: String,
    pub fragment: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SqlMetadata {
    pub dialect: SqlDialect,
    /// Human-readable SQL formatted for diagnostics and development output.
    pub text: String,
    /// Semantically identical single-line SQL for compact generated output.
    pub compact_text: String,
    pub parameters: Vec<SqlParameterMetadata>,
    pub variants: Vec<SqlVariantMetadata>,
}

#[derive(Clone, Debug, Facet)]
pub struct SqlParameterMetadata {
    pub path: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SqlVariantMetadata {
    pub path: String,
    pub cases: Vec<SqlVariantCaseMetadata>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub null_text: Option<String>,
}

#[derive(Clone, Debug, Facet)]
pub struct SqlVariantCaseMetadata {
    pub value: String,
    pub text: String,
}

#[derive(Clone, Debug, Facet)]
pub struct ResultShape {
    pub fields: Vec<ResultField>,
}

#[derive(Clone, Debug, Facet)]
pub struct ResultField {
    pub path: String,
    pub name: String,
    pub parent_path: String,
    pub kind: ResultFieldKind,
    pub value_type: ResultValueTypeMetadata,
    pub nullable: bool,
    /// `unconditional`, `context_only`, or `row_dependent`.
    pub access: String,
}

/// Public host-value contract for one result field.
#[derive(Clone, Debug, Facet)]
pub struct ResultValueTypeMetadata {
    /// `scalar`, `database_array`, or `object`.
    pub shape: ResultValueShape,
    /// Effective logical leaf name, or `object` for relation objects.
    pub name: String,
    /// Provider spelling retained for generated documentation.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub display: Option<String>,
    /// JSON representation of the scalar or database-array element.
    pub wire: WireMetadata,
}

#[derive(Clone, Debug, Facet)]
pub struct InputField {
    pub path: String,
    pub data_type: String,
    pub wire: WireMetadata,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub collection: Option<bool>,
    pub enum_values: Vec<String>,
    pub required: bool,
    pub nullable: bool,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub default: Option<InputDefault>,
}

/// Public JSON encoding plus an optional PostgreSQL text-cast target.
#[derive(Clone, Debug, Facet)]
pub struct WireMetadata {
    pub encoding: WireEncoding,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub provider_type: Option<ProviderTypeMetadata>,
}

/// Schema-qualified PostgreSQL type identity used only for generated casts.
#[derive(Clone, Debug, Facet)]
pub struct ProviderTypeMetadata {
    pub schema: String,
    pub name: String,
}

/// A typed compile-time replacement for one omitted public input.
#[derive(Clone, Debug, Facet)]
pub struct InputDefault {
    /// `string`, `number`, `boolean`, `null`, `collection`, or `empty_object`.
    pub kind: String,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub value: Option<String>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub boolean: Option<bool>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub items: Option<Vec<InputDefault>>,
}

/// One normalized bounded input capability and all of its SQL usage sites.
#[derive(Clone, Debug, Facet)]
pub struct DynamicInputMetadata {
    pub path: String,
    pub kind: String,
    pub surface: String,
    pub fields: Vec<DynamicInputField>,
    pub sites: Vec<DynamicInputSite>,
}

/// One public selected-field capability exposed by a dynamic input.
#[derive(Clone, Debug, Facet)]
pub struct DynamicInputField {
    pub key: String,
    pub catalog_path: String,
    pub data_type: String,
    pub wire: WireMetadata,
    pub nullable: bool,
    pub access: String,
    pub operators: Vec<String>,
    pub directions: Vec<String>,
}

/// One exact compiler-owned marker and its site-specific lowerings.
#[derive(Clone, Debug, Facet)]
pub struct DynamicInputSite {
    pub marker: String,
    pub identity_sql: String,
    pub fields: Vec<DynamicInputSiteField>,
}

/// Compiler-owned SQL capabilities for one public field at one usage site.
#[derive(Clone, Debug, Facet)]
pub struct DynamicInputSiteField {
    pub key: String,
    pub operators: Vec<DynamicPredicateOperatorMetadata>,
    pub directions: Vec<SqlVariantCaseMetadata>,
}

/// Structured SQL fragments for one validated dynamic predicate operator.
#[derive(Clone, Debug, Facet)]
pub struct DynamicPredicateOperatorMetadata {
    pub name: String,
    pub value_kind: String,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub before_value: Option<String>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub after_value: Option<String>,
    pub cases: Vec<SqlVariantCaseMetadata>,
}

#[derive(Clone, Debug, Facet)]
pub struct PolicyMetadata {
    /// Effective consumer scope, matching one `dsql.lock` record.
    pub scope: String,
    /// Scope that owns the filter declaration.
    pub defined_in: String,
    pub name: String,
    pub default_active: bool,
    /// `none`, `always`, or `conditional`.
    pub enforcement: String,
    pub conditions: Vec<String>,
    pub context: Vec<String>,
    pub source: SourceMapEntry,
    pub applications: Vec<PolicyApplicationMetadata>,
}

#[derive(Clone, Debug, Facet)]
pub struct PolicyApplicationMetadata {
    pub path: String,
    pub target: String,
    /// `default`, `enabled`, `disabled`, or `conditional`.
    pub assignment: String,
    pub rows_filtered: bool,
    pub fields: Vec<PolicyFieldAccessMetadata>,
}

#[derive(Clone, Debug, Facet)]
pub struct PolicyFieldAccessMetadata {
    pub name: String,
    /// `column` or `relation`.
    pub kind: String,
    /// `context_only` or `row_dependent`.
    pub access: String,
}

#[derive(Clone, Debug, Facet)]
pub struct HandoffMetadata {
    pub name: String,
    pub parent_path: String,
    pub parent_identity: Vec<String>,
    pub relation: String,
    pub child_params: Vec<String>,
    pub provided_context: Vec<ProvidedContextMetadata>,
}

#[derive(Clone, Debug, Facet)]
pub struct ProvidedContextMetadata {
    pub name: String,
    pub source_path: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SourceMapEntry {
    pub id: String,
    pub file: String,
    pub range: SourceRange,
    /// For embedded definitions: the exact byte range of the document's
    /// content between the host's backticks (the extractor's `content`
    /// capture). Absent for plain `.dsql` files. Half-open UTF-8 byte
    /// offsets into the host file; identical across definitions from the
    /// same embedded expression. Required for embedded sources in manifest
    /// version 5; absent for standalone `.dsql` sources.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub content_range: Option<SourceRange>,
}

#[derive(Clone, Copy, Debug, Facet)]
pub struct SourceRange {
    pub start: u32,
    pub end: u32,
}

#[cfg(feature = "render")]
pub fn build_manifest_json_schema() -> Result<String, String> {
    let mut generated_schema = facet_json_schema::schema_for::<BuildManifest>();
    generated_schema.schema = Some(JSON_SCHEMA_DRAFT_2020_12.to_string());
    let schema = BuildManifestJsonSchema {
        id: BUILD_MANIFEST_SCHEMA_ID,
        schema: generated_schema,
    };
    facet_json::to_string_pretty(&schema).map_err(|error| error.to_string())
}

#[cfg(feature = "render")]
pub fn build_manifest_typescript() -> String {
    let mut generator = facet_typescript::TypeScriptGenerator::new();
    generator.add_type::<FragmentMetadata>();
    generator.add_type::<OperationMetadata>();
    generator.add_type::<BuildManifest>();
    let mut typescript = format!(
        "export const BUILD_MANIFEST_VERSION = {BUILD_MANIFEST_VERSION} as const;\n\n{}",
        generator.finish()
    );
    typescript.truncate(typescript.trim_end().len());
    typescript.push('\n');
    typescript
}
