// @ts-nocheck
import {
  queryOptions as tanStackQueryOptions,
  useQuery as useTanStackQuery,
} from "@tanstack/react-query";
import type { UseQueryOptions, UseQueryResult } from "@tanstack/react-query";
import type {
  DsqlOperation,
  DsqlOperationResult,
  DsqlVariables,
} from "@dsql/typescript/runtime";
import { dsqlQueryKey } from "@dsql/typescript/runtime";
import type { DsqlServerVariables } from "./tanstack-start";

export type DsqlQueryVariables<
  Params,
  Input,
> = {
  readonly params: Params;
  readonly input: Input;
};

type DsqlVariableOptions<Params, Input> =
  DsqlVariables<DsqlOperation<unknown, Params, Input>>;

type DsqlOptionsArgument<Params, Input, Context, Options> =
  [Context] extends [Record<string, never>]
    ? {} extends DsqlVariableOptions<Params, Input>
      ? readonly [options?: Options]
      : readonly [options: Options]
    : readonly [options: Options];

type DsqlContextScopeOptions<Context> =
  [Context] extends [Record<string, never>]
    ? { readonly contextScope?: string }
    : { readonly contextScope: string };

export type DsqlQueryKey<Variables> = readonly [
  "dsql",
  string,
  string | null,
  Variables,
];

/**
 * `contextScope` is an opaque, non-secret identity for the trusted server
 * context. It participates only in the client cache key and is never sent to
 * the server function.
 */

export type DsqlQueryOptions<
  Params,
  Input,
  Context,
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
} & DsqlVariableOptions<Params, Input> & DsqlContextScopeOptions<Context>;

export type DsqlExecuteOptions<
  Params,
  Input,
> = DsqlVariableOptions<Params, Input>;

type DsqlServerFunction<Operation extends DsqlOperation<any, any, any, any>> = (
  options: {
    readonly data: DsqlServerVariables<Operation>;
  },
) => Promise<DsqlOperationResult<Operation>>;

export function queryKey<Variables>(
  operation: DsqlOperation<any, any, any, any>,
  variables: Variables,
  contextScope?: string,
): DsqlQueryKey<Variables> {
  return dsqlQueryKey(operation, variables, contextScope);
}

export function queryOptions<Result, Params, Input, Context>(
  operation: DsqlOperation<Result, Params, Input, Context>,
  ...args: DsqlOptionsArgument<
    NoInfer<Params>,
    NoInfer<Input>,
    NoInfer<Context>,
    DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Context>, NoInfer<Result>>
  >
) {
  const [options = {} as DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Context>, NoInfer<Result>>] = args;
  const {
    contextScope,
    params = {} as Params,
    input = {} as Input,
    ...tanStackOptions
  } = options;
  const variables = { params, input };
  const serverFn = serverFunctionFor(operation);
  return tanStackQueryOptions({
    ...tanStackOptions,
    queryKey: queryKey(operation, variables, contextScope),
    queryFn: () => serverFn({ data: variables }),
  });
}

export function executeQuery<Result, Params, Input, Context>(
  operation: DsqlOperation<Result, Params, Input, Context>,
  ...args: DsqlOptionsArgument<
    NoInfer<Params>,
    NoInfer<Input>,
    Record<string, never>,
    DsqlExecuteOptions<NoInfer<Params>, NoInfer<Input>>
  >
): Promise<Result> {
  const [options = {} as DsqlExecuteOptions<NoInfer<Params>, NoInfer<Input>>] = args;
  const {
    params = {} as Params,
    input = {} as Input,
  } = options;
  const serverFn = serverFunctionFor(operation);
  return serverFn({ data: { params, input } });
}

export function useQuery<Result, Params, Input, Context>(
  operation: DsqlOperation<Result, Params, Input, Context>,
  ...args: DsqlOptionsArgument<
    NoInfer<Params>,
    NoInfer<Input>,
    NoInfer<Context>,
    DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Context>, NoInfer<Result>>
  >
): UseQueryResult<Result, Error> {
  const [options = {} as DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Context>, NoInfer<Result>>] = args;
  const {
    contextScope,
    params = {} as Params,
    input = {} as Input,
    ...tanStackOptions
  } = options;
  const variables = { params, input };
  const serverFn = serverFunctionFor(operation);
  return useTanStackQuery(
    tanStackQueryOptions({
      ...tanStackOptions,
      queryKey: queryKey(operation, variables, contextScope),
      queryFn: () => serverFn({ data: variables }),
    }),
  ) as UseQueryResult<Result, Error>;
}

function serverFunctionFor<Operation extends DsqlOperation<any, any, any, any>>(
  operation: Operation,
): DsqlServerFunction<Operation> {
  const serverFn = serverFunctions[operation.name];
  if (!serverFn) {
    throw new Error(`missing dsql server function for ${operation.name}`);
  }

  return serverFn as DsqlServerFunction<Operation>;
}
