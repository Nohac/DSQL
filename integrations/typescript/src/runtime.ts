export type DsqlOperation<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
  Context = Record<string, never>,
> = {
  readonly id: string;
  readonly name: string;
  readonly kind: "query";
  /** Whether server execution binds at least one trusted-context value. */
  readonly requiresContext: boolean;
  /** Public input metadata used for defaults and stable client cache keys. */
  readonly inputs: readonly DsqlInputField[];
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
  readonly context?: Context;
};

export type DsqlOperationResult<Operation> =
  Operation extends DsqlOperation<infer Result, any, any, any> ? Result : never;

export type DsqlOperationParams<Operation> =
  Operation extends DsqlOperation<any, infer Params, any, any>
    ? Params
    : Record<string, never>;

export type DsqlOperationInput<Operation> =
  Operation extends DsqlOperation<any, any, infer Input, any>
    ? Input
    : Record<string, never>;

export type DsqlOperationContext<Operation> =
  Operation extends DsqlOperation<any, any, any, infer Context>
    ? Context
    : Record<string, never>;

export type DsqlVariables<
  Operation extends DsqlOperation<any, any, any, any>,
> = ({} extends DsqlOperationParams<Operation>
  ? { readonly params?: DsqlOperationParams<Operation> }
  : { readonly params: DsqlOperationParams<Operation> }) &
  ({} extends DsqlOperationInput<Operation>
    ? { readonly input?: DsqlOperationInput<Operation> }
    : { readonly input: DsqlOperationInput<Operation> });

export type DsqlQueryKey<Variables> = readonly [
  "dsql",
  string,
  string | null,
  Variables,
];

export function dsqlQueryKey<Variables>(
  operation: DsqlOperation<any, any, any, any>,
  variables: Variables,
  contextScope?: string,
): DsqlQueryKey<Variables> {
  if (operation.requiresContext && contextScope === undefined) {
    throw new Error(
      `dsql operation ${operation.name} requires contextScope for cache identity`,
    );
  }
  const materialized = materializeDsqlBindings(operation.inputs, variables);
  return [
    "dsql",
    operation.name,
    contextScope ?? null,
    materialized as Variables,
  ] as const;
}

export type DsqlFragmentDefinition<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
> = {
  readonly name: string;
  readonly kind: "fragment";
  readonly table: string;
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
};

export type DsqlFragment<Fragment> =
  Fragment extends DsqlFragmentDefinition<infer Result, any, any>
    ? Result
    : never;

export type DsqlFragmentParams<Fragment> =
  Fragment extends DsqlFragmentDefinition<any, infer Params, any>
    ? Params
    : Record<string, never>;

export type DsqlFragmentInput<Fragment> =
  Fragment extends DsqlFragmentDefinition<any, any, infer Input>
    ? Input
    : Record<string, never>;

export type DsqlFragmentVariables<Fragment> =
  ({} extends DsqlFragmentParams<Fragment>
    ? { readonly params?: DsqlFragmentParams<Fragment> }
    : { readonly params: DsqlFragmentParams<Fragment> }) &
  ({} extends DsqlFragmentInput<Fragment>
    ? { readonly input?: DsqlFragmentInput<Fragment> }
    : { readonly input: DsqlFragmentInput<Fragment> });

export type DsqlDefinition = DsqlOperation | DsqlFragmentDefinition;

export interface DsqlSourceRegistry {}

export type DsqlDefinitionForSource<Source extends string> =
  Source extends keyof DsqlSourceRegistry
    ? DsqlSourceRegistry[Source]
    : DsqlDefinition;

export type DsqlSqlParameter = {
  readonly path: string;
};

export type DsqlSqlVariants = Record<
  string,
  {
    readonly cases: Record<string, string>;
    readonly nullText?: string;
  }
>;

export type DsqlInputDefault = {
  readonly kind: string;
  readonly value?: string;
  readonly boolean?: boolean;
  readonly items?: readonly DsqlInputDefault[];
};

export type DsqlInputField = {
  readonly path: string;
  readonly data_type: string;
  readonly collection?: boolean;
  readonly enum_values?: readonly string[];
  readonly required: boolean;
  readonly nullable: boolean;
  readonly default?: DsqlInputDefault;
  /** Canonical identity for a nullable bounded dynamic input. */
  readonly null_identity?: "empty_object" | "empty_collection";
};

export type DsqlExecutionPayload<
  Operation extends DsqlOperation<any, any, any, any> = DsqlOperation<any, any, any, any>,
> = {
  readonly operation: Operation;
  readonly sql: string;
  readonly parameters: readonly DsqlSqlParameter[];
  readonly variants: DsqlSqlVariants;
  readonly inputs: readonly DsqlInputField[];
};

export type DsqlMaterializedQuery = {
  readonly sql: string;
  readonly values: readonly unknown[];
};

export function dsql<const Source extends string>(
  source: Source,
): DsqlDefinitionForSource<Source>;
export function dsql(
  strings: TemplateStringsArray,
  ...values: never[]
): DsqlDefinition;
export function dsql(): DsqlDefinition {
  // Callsite detection is Rust-extractor-owned and rewriting is the
  // build binding's job (docs/spec/build-daemon.md, Callsites): an
  // expression reaching this function at runtime means no binding
  // transformed the module, and evaluating the untyped fallback would
  // silently ship a broken operation.
  throw new Error(
    "dsql callsite was not transformed - is the dsql build plugin installed " +
      "and running before other transforms?",
  );
}

export function materializeDsqlQuery<
  Operation extends DsqlOperation<any, any, any, any>,
>(
  payload: DsqlExecutionPayload<Operation>,
  variables: DsqlVariables<Operation>,
  context: DsqlOperationContext<Operation>,
): DsqlMaterializedQuery {
  const bindings = materializeDsqlBindings(payload.inputs, {
    ...variables,
    context,
  });
  const sql = applyDsqlVariants(payload.sql, payload.variants, bindings);
  return {
    sql,
    values: collectDsqlParameterValues(payload.parameters, bindings),
  };
}

export function applyDsqlVariants(
  sql: string,
  variants: DsqlSqlVariants,
  variables: unknown,
): string {
  let rendered = sql;
  for (const [path, variant] of Object.entries(variants)) {
    const selected = getDsqlPath(variables, path);
    const nullText = variant.nullText;
    if (selected == null && nullText !== undefined) {
      rendered = rendered.replaceAll(`{{${path}}}`, () => nullText);
      continue;
    }
    if (typeof selected !== "string") {
      throw new Error(`missing dsql variant value at ${path}`);
    }

    const replacement = variant.cases[selected];
    if (replacement === undefined) {
      throw new Error(`invalid dsql variant value at ${path}: ${selected}`);
    }
    rendered = rendered.replaceAll(`{{${path}}}`, () => replacement);
  }
  return rendered;
}

export function materializeDsqlBindings(
  fields: readonly DsqlInputField[],
  bindings: unknown,
): unknown {
  let materialized = bindings;
  for (const field of fields) {
    const trustedContext = field.path.startsWith("context.");
    const lookup = lookupDsqlPath(materialized, field.path);
    if (lookup.kind === "invalid") {
      throw new Error(`invalid dsql input envelope at ${field.path}`);
    }

    let value = lookup.kind === "found" ? lookup.value : undefined;
    if (lookup.kind === "missing") {
      if (trustedContext) {
        throw new Error(`missing trusted dsql context at ${field.path}`);
      }
      if (field.default !== undefined) {
        value = dsqlDefaultValue(field, field.default);
        materialized = setDsqlPath(materialized, field.path, value);
      } else if (field.required) {
        throw new Error(`missing dsql input at ${field.path}`);
      }
    }

    if (value === null && !field.nullable) {
      throw new Error(
        trustedContext
          ? `missing trusted dsql context at ${field.path}`
          : `non-null dsql input is null at ${field.path}`,
      );
    }
    if (value === null && field.null_identity !== undefined) {
      const identity = field.null_identity === "empty_object" ? {} : [];
      materialized = setDsqlPath(materialized, field.path, identity);
    }
    validateDsqlSafeInteger(field, value);
  }
  return materialized;
}

function validateDsqlSafeInteger(
  field: DsqlInputField,
  value: unknown,
): void {
  if (value === null || value === undefined || field.data_type !== "int") {
    return;
  }
  if (field.collection === true) {
    if (
      !Array.isArray(value) ||
      !value.every((item) => item === null || Number.isSafeInteger(item))
    ) {
      throw new Error(
        `operation input ${field.path} must be an array of safe integers`,
      );
    }
    return;
  }
  if (!Number.isSafeInteger(value)) {
    throw new Error(`operation input ${field.path} must be a safe integer`);
  }
}

function dsqlDefaultValue(
  field: DsqlInputField,
  defaultValue: DsqlInputDefault,
): unknown {
  switch (defaultValue.kind) {
    case "string": {
      if (field.collection === true || defaultValue.value === undefined) {
        return invalidDsqlDefault(field, "a valid string default");
      }
      if (
        field.enum_values !== undefined &&
        field.enum_values.length > 0 &&
        !field.enum_values.includes(defaultValue.value)
      ) {
        return invalidDsqlDefault(field, "a declared string default");
      }
      return defaultValue.value;
    }
    case "number": {
      if (field.collection === true || defaultValue.value === undefined) {
        return invalidDsqlDefault(field, "a valid number default");
      }
      if (field.data_type === "int") {
        if (!/^[+-]?\d+$/.test(defaultValue.value)) {
          return invalidDsqlDefault(field, "an integer default");
        }
        const integer = BigInt(defaultValue.value);
        if (
          integer < BigInt(Number.MIN_SAFE_INTEGER) ||
          integer > BigInt(Number.MAX_SAFE_INTEGER)
        ) {
          return invalidDsqlDefault(field, "a safe integer default");
        }
        return Number(integer);
      }
      if (field.data_type === "float") {
        const value = Number(defaultValue.value);
        if (
          !/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(
            defaultValue.value,
          ) ||
          !Number.isFinite(value)
        ) {
          return invalidDsqlDefault(field, "a finite float default");
        }
        return value;
      }
      if (field.data_type === "numeric") {
        if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(defaultValue.value)) {
          return invalidDsqlDefault(field, "a numeric default");
        }
        return defaultValue.value;
      }
      return invalidDsqlDefault(field, "a compatible number default");
    }
    case "boolean": {
      if (
        field.collection === true ||
        field.data_type !== "boolean" ||
        defaultValue.boolean === undefined
      ) {
        return invalidDsqlDefault(field, "a valid boolean default");
      }
      return defaultValue.boolean;
    }
    case "null":
      return null;
    case "collection": {
      if (field.collection !== true || defaultValue.items === undefined) {
        return invalidDsqlDefault(field, "a valid collection default");
      }
      const itemField = { ...field, collection: false };
      return defaultValue.items.map((item) => {
        if (["null", "collection", "empty_object"].includes(item.kind)) {
          return invalidDsqlDefault(field, "a valid collection default");
        }
        return dsqlDefaultValue(itemField, item);
      });
    }
    case "empty_object":
      if (field.collection === true) {
        return invalidDsqlDefault(field, "a compatible object default");
      }
      return {};
    default:
      return invalidDsqlDefault(field, "a recognized input default");
  }
}

function invalidDsqlDefault(field: DsqlInputField, expected: string): never {
  throw new Error(`invalid dsql default at ${field.path}: expected ${expected}`);
}

function setDsqlPath(value: unknown, path: string, replacement: unknown): unknown {
  const parts = path.split(".");
  return setDsqlPathParts(value, parts, 0, replacement, path);
}

function setDsqlPathParts(
  value: unknown,
  parts: readonly string[],
  index: number,
  replacement: unknown,
  path: string,
): unknown {
  if (!isDsqlEnvelope(value)) {
    throw new Error(`cannot materialize dsql input at ${path}`);
  }
  const part = parts[index];
  if (part === undefined) {
    return replacement;
  }
  if (index === parts.length - 1) {
    return { ...value, [part]: replacement };
  }
  const child = value[part] === undefined ? {} : value[part];
  if (!isDsqlEnvelope(child)) {
    throw new Error(`cannot materialize dsql input at ${path}`);
  }
  return {
    ...value,
    [part]: setDsqlPathParts(child, parts, index + 1, replacement, path),
  };
}

export function collectDsqlParameterValues(
  parameters: readonly DsqlSqlParameter[],
  variables: unknown,
): readonly unknown[] {
  return parameters.map((parameter) => getDsqlPath(variables, parameter.path));
}

export function getDsqlPath(value: unknown, path: string): unknown {
  const lookup = lookupDsqlPath(value, path);
  return lookup.kind === "found" ? lookup.value : undefined;
}

type DsqlPathLookup =
  | { readonly kind: "found"; readonly value: unknown }
  | { readonly kind: "missing" }
  | { readonly kind: "invalid" };

function lookupDsqlPath(value: unknown, path: string): DsqlPathLookup {
  if (path === "") {
    return { kind: "found", value };
  }

  let current = value;
  for (const part of path.split(".")) {
    if (!isDsqlEnvelope(current)) {
      return { kind: "invalid" };
    }
    if (!Object.prototype.hasOwnProperty.call(current, part)) {
      return { kind: "missing" };
    }
    current = current[part];
    if (current === undefined) {
      return { kind: "missing" };
    }
  }
  return { kind: "found", value: current };
}

function isDsqlEnvelope(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
