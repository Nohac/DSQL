export type DsqlOperation<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
> = {
  readonly id: string;
  readonly name: string;
  readonly kind: "query";
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
};

export type DsqlOperationResult<Operation> =
  Operation extends DsqlOperation<infer Result, any, any> ? Result : never;

export type DsqlOperationParams<Operation> =
  Operation extends DsqlOperation<any, infer Params, any>
    ? Params
    : Record<string, never>;

export type DsqlOperationInput<Operation> =
  Operation extends DsqlOperation<any, any, infer Input>
    ? Input
    : Record<string, never>;

export type DsqlVariables<
  Operation extends DsqlOperation<any, any, any>,
> = {
  readonly params: DsqlOperationParams<Operation>;
  readonly input: DsqlOperationInput<Operation>;
};

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
  (DsqlFragmentParams<Fragment> extends Record<string, never>
    ? { readonly params?: DsqlFragmentParams<Fragment> }
    : { readonly params: DsqlFragmentParams<Fragment> }) &
  (DsqlFragmentInput<Fragment> extends Record<string, never>
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

export type DsqlSqlVariants = Record<string, Record<string, string>>;

export type DsqlExecutionPayload<
  Operation extends DsqlOperation<any, any, any> = DsqlOperation<any, any, any>,
> = {
  readonly operation: Operation;
  readonly sql: string;
  readonly parameters: readonly DsqlSqlParameter[];
  readonly variants: DsqlSqlVariants;
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
  Operation extends DsqlOperation<any, any, any>,
>(
  payload: DsqlExecutionPayload<Operation>,
  variables: DsqlVariables<Operation>,
): DsqlMaterializedQuery {
  const sql = applyDsqlVariants(payload.sql, payload.variants, variables);
  return {
    sql,
    values: collectDsqlParameterValues(payload.parameters, variables),
  };
}

export function applyDsqlVariants(
  sql: string,
  variants: DsqlSqlVariants,
  variables: unknown,
): string {
  let rendered = sql;
  for (const [path, cases] of Object.entries(variants)) {
    const selected = getDsqlPath(variables, path);
    if (typeof selected !== "string") {
      throw new Error(`missing dsql variant value at ${path}`);
    }

    const replacement = cases[selected];
    if (replacement === undefined) {
      throw new Error(`invalid dsql variant value at ${path}: ${selected}`);
    }
    rendered = rendered.replaceAll(`{{${path}}}`, replacement);
  }
  return rendered;
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
