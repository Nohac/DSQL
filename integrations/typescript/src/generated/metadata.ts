export interface BuildManifest {
  version: number;
  operations: OperationManifestEntry[];
  fragments: FragmentManifestEntry[];
}

export interface FragmentManifestEntry {
  name: string;
  kind: string;
  path: string;
  hash: string;
  source: string;
}

export interface OperationManifestEntry {
  name: string;
  kind: string;
  path: string;
  hash: string;
  source: string;
}


export interface OperationMetadata {
  name: string;
  kind: string;
  sql: SqlMetadata;
  result: ResultShape;
  params: InputField[];
  input: InputField[];
  context: InputField[];
  dynamic_inputs: DynamicInputMetadata[];
  policies: PolicyMetadata[];
  handoffs: HandoffMetadata[];
  fragment_spreads: FragmentSpreadMetadata[];
  source_map: SourceMapEntry[];
}

export interface SourceMapEntry {
  id: string;
  file: string;
  range: SourceRange;
}

export interface SourceRange {
  start: number;
  end: number;
}

export interface FragmentSpreadMetadata {
  path: string;
  fragment: string;
}

export interface HandoffMetadata {
  name: string;
  parent_path: string;
  parent_identity: string[];
  relation: string;
  child_params: string[];
  provided_context: ProvidedContextMetadata[];
}

export interface ProvidedContextMetadata {
  name: string;
  source_path: string;
}

export interface PolicyMetadata {
  name: string;
  kind: string;
  targets: string[];
  context: string[];
}

export interface DynamicInputMetadata {
  name: string;
  kind: string;
  preset: string;
  fields: DynamicInputField[];
}

export interface DynamicInputField {
  path: string;
  data_type: string;
  operators: string[];
}

export interface InputField {
  path: string;
  data_type: string;
  enum_values: string[];
  required: boolean;
  nullable: boolean;
}

export interface ResultShape {
  fields: ResultField[];
}

export interface ResultField {
  path: string;
  name: string;
  parent_path: string;
  kind: string;
  data_type: string;
  nullable: boolean;
}

export interface SqlMetadata {
  dialect: string;
  text: string;
  parameters: SqlParameterMetadata[];
  variants: SqlVariantMetadata[];
}

export interface SqlVariantMetadata {
  path: string;
  cases: SqlVariantCaseMetadata[];
}

export interface SqlVariantCaseMetadata {
  value: string;
  text: string;
}

export interface SqlParameterMetadata {
  path: string;
}


export interface FragmentMetadata {
  name: string;
  kind: string;
  table: string;
  result: ResultShape;
  params: InputField[];
  input: InputField[];
  dynamic_inputs: DynamicInputMetadata[];
  source_map: SourceMapEntry[];
}

export interface SourceMapEntry {
  id: string;
  file: string;
  range: SourceRange;
}

export interface SourceRange {
  start: number;
  end: number;
}

export interface DynamicInputMetadata {
  name: string;
  kind: string;
  preset: string;
  fields: DynamicInputField[];
}

export interface DynamicInputField {
  path: string;
  data_type: string;
  operators: string[];
}

export interface InputField {
  path: string;
  data_type: string;
  enum_values: string[];
  required: boolean;
  nullable: boolean;
}

export interface ResultShape {
  fields: ResultField[];
}

export interface ResultField {
  path: string;
  name: string;
  parent_path: string;
  kind: string;
  data_type: string;
  nullable: boolean;
}
