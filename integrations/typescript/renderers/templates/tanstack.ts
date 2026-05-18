export const tanstackQueryTemplate = String.raw`import type {
  DsqlOperation,
  DsqlOperationInput,
  DsqlOperationParams,
  DsqlOperationResult,
} from "./operations";

export type DsqlQueryKey<Variables> = readonly ["dsql", string, Variables];

export type DsqlQueryResult<Result> = {
  readonly data: Result | undefined;
  readonly error: unknown;
  readonly isError: boolean;
  readonly isLoading: boolean;
  readonly isPending: boolean;
  readonly isSuccess: boolean;
};

export type DsqlQueryOptions<Params, Input, Result> = {
  readonly params?: Params;
  readonly input?: Input;
  readonly enabled?: boolean;
  readonly staleTime?: number;
  readonly gcTime?: number;
  readonly retry?: boolean | number;
  readonly select?: (result: Result) => unknown;
};

export type DsqlTanStackQueryOptions<Variables, Result> = Omit<
  DsqlQueryOptions<unknown, unknown, Result>,
  "params" | "input"
> & {
  readonly queryKey: DsqlQueryKey<Variables>;
  readonly queryFn: () => Promise<Result>;
};

export type DsqlTanStackUseQuery = <Variables, Result>(
  options: DsqlTanStackQueryOptions<Variables, Result>,
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

export function queryKey<Variables>(
  operation: DsqlOperation,
  variables: Variables,
): DsqlQueryKey<Variables> {
  return ["dsql", operation.name, variables] as const;
}

export function useQuery<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  options: DsqlQueryOptions<
    DsqlOperationParams<Operation>,
    DsqlOperationInput<Operation>,
    DsqlOperationResult<Operation>
  >,
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
`;

export function tanstackStartTemplate(operationNames: readonly string[]): string {
  return `export const serverOperationNames = ${JSON.stringify(operationNames)} as const;
${String.raw`import type {
  DsqlOperation,
  DsqlOperationInput,
  DsqlOperationParams,
  DsqlOperationResult,
} from "./operations";

export type DsqlServerVariables<Operation extends DsqlOperation<any, any, any>> = {
  readonly params: DsqlOperationParams<Operation>;
  readonly input: DsqlOperationInput<Operation>;
};

export type DsqlServerExecutor = <
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: Operation,
  variables: DsqlServerVariables<Operation>,
) => Promise<DsqlOperationResult<Operation>>;

export type TanStackStartServerFnFactory<ServerFn> = <
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: Operation,
  handler: (variables: DsqlServerVariables<Operation>) => Promise<DsqlOperationResult<Operation>>,
) => ServerFn;

export function createDsqlServerFunctionFactory<ServerFn>(
  createServerFunction: TanStackStartServerFnFactory<ServerFn>,
  execute: DsqlServerExecutor,
): TanStackStartServerFnFactory<ServerFn> {
  return (operation, handler = (variables) => execute(operation, variables)) =>
    createServerFunction(operation, handler);
}
`}`;
}
