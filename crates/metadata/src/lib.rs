use facet::Facet;

#[derive(Clone, Debug, Facet)]
#[facet(facet_jsonschema::id = "https://dsql.dev/schemas/build-manifest.schema.json")]
pub struct BuildManifest {
    pub version: u32,
    pub operations: Vec<OperationManifestEntry>,
}

#[derive(Clone, Debug, Facet)]
pub struct OperationManifestEntry {
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
    pub source_map: Vec<SourceMapEntry>,
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
}

#[derive(Clone, Copy, Debug, Facet)]
pub struct SourceRange {
    pub start: u32,
    pub end: u32,
}

pub fn build_manifest_json_schema() -> String {
    facet_jsonschema::to_string::<BuildManifest>()
}

pub fn build_manifest_typescript() -> String {
    let mut typescript = facet_typescript::to_typescript::<BuildManifest>();
    typescript.push('\n');
    typescript.push_str(&facet_typescript::to_typescript::<OperationMetadata>());
    typescript
}
