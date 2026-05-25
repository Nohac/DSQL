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

type DsqlOptionalParams<Params> =
  [Params] extends [Record<string, never>]
    ? { readonly params?: Params }
    : { readonly params: Params };

type DsqlOptionalInput<Input> =
  [Input] extends [Record<string, never>]
    ? { readonly input?: Input }
    : { readonly input: Input };

type DsqlVariableOptions<Params, Input> =
  DsqlOptionalParams<Params> & DsqlOptionalInput<Input>;

type DsqlOptionsArgument<Params, Input, Options> =
  [Params] extends [Record<string, never>]
    ? [Input] extends [Record<string, never>]
      ? readonly [options?: Options]
      : readonly [options: Options]
    : readonly [options: Options];

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
} & DsqlVariableOptions<Params, Input>;

export type DsqlExecuteOptions<
  Params,
  Input,
> = DsqlVariableOptions<Params, Input>;

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
  ...args: DsqlOptionsArgument<
    NoInfer<Params>,
    NoInfer<Input>,
    DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Result>>
  >
) {
  const [options = {} as DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Result>>] = args;
  const {
    params = {} as Params,
    input = {} as Input,
    ...tanStackOptions
  } = options;
  const variables = { params, input };
  return tanStackQueryOptions({
    ...tanStackOptions,
    queryKey: queryKey(operation, variables),
    queryFn: () => executeQuery(operation, { params, input }),
  });
}

export function executeQuery<Result, Params, Input>(
  operation: DsqlOperation<Result, Params, Input>,
  ...args: DsqlOptionsArgument<
    NoInfer<Params>,
    NoInfer<Input>,
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

export function useQuery<Result, Params, Input>(
  operation: DsqlOperation<Result, Params, Input>,
  ...args: DsqlOptionsArgument<
    NoInfer<Params>,
    NoInfer<Input>,
    DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Result>>
  >
): UseQueryResult<Result, Error> {
  const [options = {} as DsqlQueryOptions<NoInfer<Params>, NoInfer<Input>, NoInfer<Result>>] = args;
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
