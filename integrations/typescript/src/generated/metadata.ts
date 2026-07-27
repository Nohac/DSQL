export const BUILD_MANIFEST_VERSION = 4 as const;

export interface BuildManifest {
  version: number;
  /**
   * The project-monotonic generation this manifest commits
   * (docs/spec/build-daemon.md).
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
   * same embedded expression. Required for embedded sources in manifest
   * version 3; absent for standalone `.dsql` sources.
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

/** One normalized bounded input capability and all of its SQL usage sites. */
export interface DynamicInputMetadata {
  path: string;
  kind: string;
  surface: string;
  fields: DynamicInputField[];
  sites: DynamicInputSite[];
}

/** One exact compiler-owned marker and its site-specific lowerings. */
export interface DynamicInputSite {
  marker: string;
  identity_sql: string;
  fields: DynamicInputSiteField[];
}

/** Compiler-owned SQL capabilities for one public field at one usage site. */
export interface DynamicInputSiteField {
  key: string;
  operators: DynamicPredicateOperatorMetadata[];
  directions: SqlVariantCaseMetadata[];
}

export interface SqlVariantCaseMetadata {
  value: string;
  text: string;
}

/** Structured SQL fragments for one validated dynamic predicate operator. */
export interface DynamicPredicateOperatorMetadata {
  name: string;
  value_kind: string;
  before_value?: string;
  after_value?: string;
  cases: SqlVariantCaseMetadata[];
}

/** One public selected-field capability exposed by a dynamic input. */
export interface DynamicInputField {
  key: string;
  catalog_path: string;
  data_type: string;
  wire: WireMetadata;
  nullable: boolean;
  access: string;
  operators: string[];
  directions: string[];
}

/** Public JSON encoding plus an optional PostgreSQL text-cast target. */
export interface WireMetadata {
  encoding: string;
  provider_type?: ProviderTypeMetadata;
}

/** Schema-qualified PostgreSQL type identity used only for generated casts. */
export interface ProviderTypeMetadata {
  schema: string;
  name: string;
}

export interface InputField {
  path: string;
  data_type: string;
  wire: WireMetadata;
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
  wire: WireMetadata;
  nullable: boolean;
  /** `unconditional`, `context_only`, or `row_dependent`. */
  access: string;
}

export interface SqlMetadata {
  dialect: string;
  /** Human-readable SQL formatted for diagnostics and development output. */
  text: string;
  /** Semantically identical single-line SQL for compact generated output. */
  compact_text: string;
  parameters: SqlParameterMetadata[];
  variants: SqlVariantMetadata[];
}

export interface SqlVariantMetadata {
  path: string;
  cases: SqlVariantCaseMetadata[];
  null_text?: string;
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
   * renderers compose fragment types by reuse from this.
   */
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
   * same embedded expression. Required for embedded sources in manifest
   * version 3; absent for standalone `.dsql` sources.
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

/** One normalized bounded input capability and all of its SQL usage sites. */
export interface DynamicInputMetadata {
  path: string;
  kind: string;
  surface: string;
  fields: DynamicInputField[];
  sites: DynamicInputSite[];
}

/** One exact compiler-owned marker and its site-specific lowerings. */
export interface DynamicInputSite {
  marker: string;
  identity_sql: string;
  fields: DynamicInputSiteField[];
}

/** Compiler-owned SQL capabilities for one public field at one usage site. */
export interface DynamicInputSiteField {
  key: string;
  operators: DynamicPredicateOperatorMetadata[];
  directions: SqlVariantCaseMetadata[];
}

export interface SqlVariantCaseMetadata {
  value: string;
  text: string;
}

/** Structured SQL fragments for one validated dynamic predicate operator. */
export interface DynamicPredicateOperatorMetadata {
  name: string;
  value_kind: string;
  before_value?: string;
  after_value?: string;
  cases: SqlVariantCaseMetadata[];
}

/** One public selected-field capability exposed by a dynamic input. */
export interface DynamicInputField {
  key: string;
  catalog_path: string;
  data_type: string;
  wire: WireMetadata;
  nullable: boolean;
  access: string;
  operators: string[];
  directions: string[];
}

/** Public JSON encoding plus an optional PostgreSQL text-cast target. */
export interface WireMetadata {
  encoding: string;
  provider_type?: ProviderTypeMetadata;
}

/** Schema-qualified PostgreSQL type identity used only for generated casts. */
export interface ProviderTypeMetadata {
  schema: string;
  name: string;
}

export interface InputField {
  path: string;
  data_type: string;
  wire: WireMetadata;
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
  wire: WireMetadata;
  nullable: boolean;
  /** `unconditional`, `context_only`, or `row_dependent`. */
  access: string;
}
