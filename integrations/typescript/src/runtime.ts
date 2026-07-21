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
  return ["dsql", operation.name, contextScope ?? null, variables] as const;
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
  readonly required: boolean;
  readonly nullable: boolean;
  readonly default?: DsqlInputDefault;
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
  for (const parameter of payload.parameters) {
    if (
      parameter.path.startsWith("context.") &&
      getDsqlPath(bindings, parameter.path) == null
    ) {
      throw new Error(`missing trusted dsql context at ${parameter.path}`);
    }
  }
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
    if (selected == null && variant.nullText !== undefined) {
      rendered = rendered.replaceAll(`{{${path}}}`, variant.nullText);
      continue;
    }
    if (typeof selected !== "string") {
      throw new Error(`missing dsql variant value at ${path}`);
    }

    const replacement = variant.cases[selected];
    if (replacement === undefined) {
      throw new Error(`invalid dsql variant value at ${path}: ${selected}`);
    }
    rendered = rendered.replaceAll(`{{${path}}}`, replacement);
  }
  return rendered;
}

export function materializeDsqlBindings(
  fields: readonly DsqlInputField[],
  bindings: unknown,
): unknown {
  const materialized = cloneDsqlValue(bindings);
  for (const field of fields) {
    let value = getDsqlPath(materialized, field.path);
    if (value === undefined && field.default !== undefined) {
      value = dsqlDefaultValue(field, field.default);
      setDsqlPath(materialized, field.path, value);
    }
    if (value === undefined && field.required) {
      if (field.path.startsWith("context.")) {
        throw new Error(`missing trusted dsql context at ${field.path}`);
      }
      throw new Error(`missing dsql input at ${field.path}`);
    }
    if (value === null && !field.nullable) {
      if (field.path.startsWith("context.")) {
        throw new Error(`missing trusted dsql context at ${field.path}`);
      }
      throw new Error(`non-null dsql input is null at ${field.path}`);
    }
  }
  return materialized;
}

function dsqlDefaultValue(
  field: DsqlInputField,
  defaultValue: DsqlInputDefault,
): unknown {
  switch (defaultValue.kind) {
    case "string":
      return defaultValue.value;
    case "number":
      if (field.data_type === "numeric") return defaultValue.value;
      return Number(defaultValue.value);
    case "boolean":
      return defaultValue.boolean;
    case "null":
      return null;
    case "collection":
      return (defaultValue.items ?? []).map((item) =>
        dsqlDefaultValue(field, item),
      );
    case "empty_object":
      return {};
    default:
      throw new Error(`invalid dsql default at ${field.path}`);
  }
}

function cloneDsqlValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(cloneDsqlValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, child]) => [key, cloneDsqlValue(child)]),
    );
  }
  return value;
}

function setDsqlPath(value: unknown, path: string, replacement: unknown): void {
  if (value === null || typeof value !== "object") {
    throw new Error(`cannot materialize dsql input at ${path}`);
  }
  const parts = path.split(".");
  let current = value as Record<string, unknown>;
  for (const part of parts.slice(0, -1)) {
    const child = current[part];
    if (child === null || typeof child !== "object" || Array.isArray(child)) {
      current[part] = {};
    }
    current = current[part] as Record<string, unknown>;
  }
  const leaf = parts.at(-1);
  if (leaf !== undefined) current[leaf] = replacement;
}

export function collectDsqlParameterValues(
  parameters: readonly DsqlSqlParameter[],
  variables: unknown,
): readonly unknown[] {
  return parameters.map((parameter) => getDsqlPath(variables, parameter.path));
}

export function getDsqlPath(value: unknown, path: string): unknown {
  if (path === "") {
    return value;
  }

  let current = value;
  for (const part of path.split(".")) {
    if (current === null || typeof current !== "object") {
      return undefined;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}
