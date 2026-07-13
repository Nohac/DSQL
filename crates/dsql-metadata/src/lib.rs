use facet::Facet;

const BUILD_MANIFEST_SCHEMA_ID: &str = "https://dsql.dev/schemas/build-manifest.schema.json";

/// Manifest format version 2: `generationId` plus content-addressed
/// artifact paths (docs/spec/build-daemon.md, Transactionality).
pub const BUILD_MANIFEST_VERSION: u32 = 2;
const JSON_SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

#[derive(Debug, Facet)]
#[facet(skip_all_unless_truthy)]
struct BuildManifestJsonSchema {
    #[facet(rename = "$id")]
    id: &'static str,
    #[facet(flatten)]
    schema: facet_json_schema::JsonSchema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum DefinitionKind {
    Query,
    Fragment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum ArtifactKind {
    Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum SqlDialect {
    Postgres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum ResultFieldKind {
    Scalar,
    Object,
    Array,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum ResultDataType {
    Object,
}

#[derive(Clone, Debug, Facet)]
pub struct BuildManifest {
    pub version: u32,
    /// The project-monotonic generation this manifest commits
    /// (docs/spec/build-daemon.md). Required in version 2; version-1
    /// pointers are read through a private compatibility reader.
    #[facet(rename = "generationId")]
    pub generation_id: u64,
    pub operations: Vec<OperationManifestEntry>,
    pub fragments: Vec<FragmentManifestEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct OperationManifestEntry {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct FragmentManifestEntry {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug, Facet)]
pub struct OperationMetadata {
    pub name: String,
    pub kind: String,
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
    pub kind: String,
    pub table: String,
    pub result: ResultShape,
    pub params: Vec<InputField>,
    pub input: Vec<InputField>,
    pub dynamic_inputs: Vec<DynamicInputMetadata>,
    pub source_map: Vec<SourceMapEntry>,
}

#[derive(Clone, Debug, Facet)]
pub struct FragmentSpreadMetadata {
    pub path: String,
    pub fragment: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SqlMetadata {
    pub dialect: String,
    pub text: String,
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
    pub kind: String,
    pub data_type: String,
    pub nullable: bool,
}

#[derive(Clone, Debug, Facet)]
pub struct InputField {
    pub path: String,
    pub data_type: String,
    pub enum_values: Vec<String>,
    pub required: bool,
    pub nullable: bool,
}

#[derive(Clone, Debug, Facet)]
pub struct DynamicInputMetadata {
    pub name: String,
    pub kind: String,
    pub preset: String,
    pub fields: Vec<DynamicInputField>,
}

#[derive(Clone, Debug, Facet)]
pub struct DynamicInputField {
    pub path: String,
    pub data_type: String,
    pub operators: Vec<String>,
}

#[derive(Clone, Debug, Facet)]
pub struct PolicyMetadata {
    pub name: String,
    pub kind: String,
    pub targets: Vec<String>,
    pub context: Vec<String>,
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
    /// same embedded expression. Additive to manifest version 2.
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub content_range: Option<SourceRange>,
}

#[derive(Clone, Copy, Debug, Facet)]
pub struct SourceRange {
    pub start: u32,
    pub end: u32,
}

pub fn build_manifest_json_schema() -> Result<String, String> {
    let mut generated_schema = facet_json_schema::schema_for::<BuildManifest>();
    generated_schema.schema = Some(JSON_SCHEMA_DRAFT_2020_12.to_string());
    let schema = BuildManifestJsonSchema {
        id: BUILD_MANIFEST_SCHEMA_ID,
        schema: generated_schema,
    };
    facet_json::to_string_pretty(&schema).map_err(|error| error.to_string())
}

pub fn build_manifest_typescript() -> String {
    let mut typescript = [
        facet_typescript::to_typescript::<BuildManifest>(),
        facet_typescript::to_typescript::<OperationMetadata>(),
        facet_typescript::to_typescript::<FragmentMetadata>(),
    ]
    .join("\n");
    typescript.truncate(typescript.trim_end().len());
    typescript.push('\n');
    typescript
}
