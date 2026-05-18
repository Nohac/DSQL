export type DsqlOperation<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
> = {
  readonly id: string;
  readonly name: string;
  readonly kind: "query";
  readonly sql: string;
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
};

export type DsqlOperationResult<Operation> =
  Operation extends DsqlOperation<infer Result> ? Result : never;

export type DsqlOperationParams<Operation> =
  Operation extends DsqlOperation<unknown, infer Params, unknown>
    ? Params
    : Record<string, never>;

export type DsqlOperationInput<Operation> =
  Operation extends DsqlOperation<unknown, unknown, infer Input>
    ? Input
    : Record<string, never>;

export type DsqlOperationForSource<Source extends string> =
  Source extends keyof DsqlOperationBySource
    ? DsqlOperationBySource[Source]
    : DsqlOperation;

export type DsqlQueryKey<Params> = readonly ["dsql", string, Params];

export type DsqlQueryResult<Result> = {
  readonly data: Result | undefined;
  readonly error: unknown;
  readonly isError: boolean;
  readonly isLoading: boolean;
  readonly isPending: boolean;
  readonly isSuccess: boolean;
};

export type DsqlQueryOptions<Params, Result> = {
  readonly params?: Params;
  readonly input?: unknown;
  readonly enabled?: boolean;
  readonly staleTime?: number;
  readonly gcTime?: number;
  readonly retry?: boolean | number;
  readonly select?: (result: Result) => unknown;
};

export type DsqlTanStackQueryOptions<Params, Result> = Omit<
  DsqlQueryOptions<Params, Result>,
  "params"
> & {
  readonly queryKey: DsqlQueryKey<Params>;
  readonly queryFn: () => Promise<Result>;
};

export type DsqlTanStackUseQuery = <Params, Result>(
  options: DsqlTanStackQueryOptions<Params, Result>,
) => DsqlQueryResult<Result>;

export type DsqlExecuteQuery = <Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  variables: {
    readonly params: DsqlOperationParams<Operation>;
    readonly input: DsqlOperationInput<Operation>;
  },
) => Promise<DsqlOperationResult<Operation>>;

export type DsqlRuntime = {
  readonly useTanStackQuery: DsqlTanStackUseQuery;
  readonly executeQuery: DsqlExecuteQuery;
};

let runtime: DsqlRuntime | undefined;

export function configureDsql(runtimeConfig: DsqlRuntime): void {
  runtime = runtimeConfig;
}

export function queryKey<Params>(
  operation: DsqlOperation,
  variables: Params,
): DsqlQueryKey<Params> {
  return ["dsql", operation.name, variables] as const;
}

export function useQuery<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  options: Omit<
    DsqlQueryOptions<
      DsqlOperationParams<Operation>,
      DsqlOperationResult<Operation>
    >,
    "input"
  > & {
    readonly input?: DsqlOperationInput<Operation>;
  },
): DsqlQueryResult<DsqlOperationResult<Operation>> {
  if (!runtime) {
    throw new Error("dsql runtime is not configured");
  }

  const {
    params = {} as DsqlOperationParams<Operation>,
    input = {} as DsqlOperationInput<Operation>,
    ...queryOptions
  } = options;
  const variables = { params, input };
  return runtime.useTanStackQuery({
    ...queryOptions,
    queryKey: queryKey(operation, variables),
    queryFn: () => runtime!.executeQuery(operation, variables),
  });
}

export function dsql<const Source extends string>(
  source: Source,
): DsqlOperationForSource<Source>;
export function dsql(strings: TemplateStringsArray, ...values: never[]): DsqlOperation;
export function dsql(
  input: string | TemplateStringsArray,
  ...values: never[]
): DsqlOperation {
  if (values.length > 0) {
    throw new Error("dsql templates do not support JavaScript interpolation");
  }

  const source = typeof input === "string" ? input : (input.raw[0] ?? "");
  const name = /\bquery\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source)?.[1];
  return {
    id: "<untransformed>",
    name: name ?? "<untransformed>",
    kind: "query",
    sql: "",
  };
}
