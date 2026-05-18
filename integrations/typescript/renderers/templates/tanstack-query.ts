// @ts-nocheck
import {
  queryOptions as tanStackQueryOptions,
  useQuery as useTanStackQuery,
} from "@tanstack/react-query";
import type { UseQueryOptions, UseQueryResult } from "@tanstack/react-query";
import type {
  DsqlOperation,
  DsqlOperationResult,
} from "./operations";
import type { DsqlServerVariables } from "./tanstack-start";

export type DsqlQueryVariables<
  Params,
  Input,
> = {
  readonly params: Params;
  readonly input: Input;
};

export type DsqlQueryKey<Variables> = readonly ["dsql", string, Variables];

export type DsqlQueryOptions<
  Params,
  Input,
  Result,
> = Omit<
  UseQueryOptions<
    Result,
    Error,
    Result,
    DsqlQueryKey<DsqlQueryVariables<Params, Input>>
  >,
  "queryKey" | "queryFn"
> & {
  readonly params?: Params;
  readonly input?: Input;
};

type DsqlServerFunction<Operation extends DsqlOperation<any, any, any>> = (
  options: {
    readonly data: DsqlServerVariables<Operation>;
  },
) => Promise<DsqlOperationResult<Operation>>;

export function queryKey<Variables>(
  operation: DsqlOperation<any, any, any>,
  variables: Variables,
): DsqlQueryKey<Variables> {
  return ["dsql", operation.name, variables] as const;
}

export function queryOptions<Result, Params, Input>(
  operation: DsqlOperation<Result, Params, Input>,
  options: DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Result>>,
) {
  const {
    params = {} as Params,
    input = {} as Input,
    ...tanStackOptions
  } = options;
  const variables = { params, input };
  const serverFn = serverFunctionFor(operation);
  return tanStackQueryOptions({
    ...tanStackOptions,
    queryKey: queryKey(operation, variables),
    queryFn: () => serverFn({ data: variables }),
  });
}

export function useQuery<Result, Params, Input>(
  operation: DsqlOperation<Result, Params, Input>,
  options: DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Result>>,
): UseQueryResult<Result, Error> {
  return useTanStackQuery(queryOptions(operation, options)) as UseQueryResult<
    Result,
    Error
  >;
}

function serverFunctionFor<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
): DsqlServerFunction<Operation> {
  const serverFn = serverFunctions[operation.name];
  if (!serverFn) {
    throw new Error(`missing dsql server function for ${operation.name}`);
  }

  return serverFn as DsqlServerFunction<Operation>;
}
