export interface BuildManifest {
  version: number;
  /**
   * The project-monotonic generation this manifest commits
   * (docs/spec/build-daemon.md). Required in version 2; version-1
   * pointers are read through a private compatibility reader.
   */
  generationId: number;
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
  /**
   * For embedded definitions: the exact byte range of the document's
   * content between the host's backticks (the extractor's `content`
   * capture). Absent for plain `.dsql` files. Half-open UTF-8 byte
   * offsets into the host file; identical across definitions from the
   * same embedded expression. Additive to manifest version 2.
   */
  content_range?: SourceRange;
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
  /** Effective consumer scope, matching one `dsql.lock` record. */
  scope: string;
  /** Scope that owns the filter declaration. */
  defined_in: string;
  name: string;
  default_active: boolean;
  /** `none`, `always`, or `conditional`. */
  enforcement: string;
  conditions: string[];
  context: string[];
  source: SourceMapEntry;
  applications: PolicyApplicationMetadata[];
}

export interface PolicyApplicationMetadata {
  path: string;
  target: string;
  /** `default`, `enabled`, `disabled`, or `conditional`. */
  assignment: string;
  rows_filtered: boolean;
  fields: PolicyFieldAccessMetadata[];
}

export interface PolicyFieldAccessMetadata {
  name: string;
  /** `column` or `relation`. */
  kind: string;
  /** `context_only` or `row_dependent`. */
  access: string;
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
  collection?: boolean;
  enum_values: string[];
  required: boolean;
  nullable: boolean;
  default?: InputDefault;
}

/** A typed compile-time replacement for one omitted public input. */
export interface InputDefault {
  /** `string`, `number`, `boolean`, `null`, `collection`, or `empty_object`. */
  kind: string;
  value?: string;
  boolean?: boolean;
  items?: InputDefault[];
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
  /** `unconditional`, `context_only`, or `row_dependent`. */
  access: string;
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
  null_text?: string;
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
  /**
   * Fragment spreads the body expanded, path-qualified like the
   * operation-level field (the empty path is the fragment root):
   * renderers compose fragment types by reuse from this. Additive to
   * manifest version 2 — readers treat absence as empty.
   */
  fragment_spreads?: FragmentSpreadMetadata[];
  source_map: SourceMapEntry[];
}

export interface SourceMapEntry {
  id: string;
  file: string;
  range: SourceRange;
  /**
   * For embedded definitions: the exact byte range of the document's
   * content between the host's backticks (the extractor's `content`
   * capture). Absent for plain `.dsql` files. Half-open UTF-8 byte
   * offsets into the host file; identical across definitions from the
   * same embedded expression. Additive to manifest version 2.
   */
  content_range?: SourceRange;
}

export interface SourceRange {
  start: number;
  end: number;
}

export interface FragmentSpreadMetadata {
  path: string;
  fragment: string;
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
  collection?: boolean;
  enum_values: string[];
  required: boolean;
  nullable: boolean;
  default?: InputDefault;
}

/** A typed compile-time replacement for one omitted public input. */
export interface InputDefault {
  /** `string`, `number`, `boolean`, `null`, `collection`, or `empty_object`. */
  kind: string;
  value?: string;
  boolean?: boolean;
  items?: InputDefault[];
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
  /** `unconditional`, `context_only`, or `row_dependent`. */
  access: string;
}
