// @ts-nocheck
import {
  queryOptions as tanStackQueryOptions,
  useQuery as useTanStackQuery,
} from "@tanstack/react-query";
import type { UseQueryOptions, UseQueryResult } from "@tanstack/react-query";
import type {
  DsqlOperation,
  DsqlOperationContext,
  DsqlOperationResult,
  DsqlOperationWireResult,
  DsqlVariables,
  DsqlWireVariables,
} from "@dsql/typescript/runtime";
import {
  dsqlQueryKey,
  dsqlQueryKeyForWire,
  dsqlResultSelector,
  materializeDsqlOperationVariables,
  parseDsqlResult,
} from "@dsql/typescript/runtime";
import type { DsqlServerVariables } from "./tanstack-start";

type AnyDsqlOperation = DsqlOperation<any, any, any, any, any>;

type DsqlOptionsArgument<Operation extends AnyDsqlOperation, Options> =
  [DsqlOperationContext<Operation>] extends [Record<string, never>]
    ? {} extends DsqlVariables<Operation>
      ? readonly [options?: Options]
      : readonly [options: Options]
    : readonly [options: Options];

type DsqlContextScopeOptions<Operation extends AnyDsqlOperation> =
  [DsqlOperationContext<Operation>] extends [Record<string, never>]
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
 *
 * TanStack stores the raw wire result in its cache and dehydration payload.
 * `select` parses configured host scalars before applying the caller selector.
 * Consequently direct cache APIs such as `getQueryData`, `ensureQueryData`,
 * and `prefetchQuery` observe the raw wire result; call `parseDsqlResult` when
 * consuming their returned data directly.
 */
export type DsqlQueryOptions<
  Operation extends AnyDsqlOperation,
  Selected = DsqlOperationResult<Operation>,
> = Omit<
  UseQueryOptions<
    DsqlOperationWireResult<Operation>,
    Error,
    Selected,
    DsqlQueryKey<DsqlWireVariables<Operation>>
  >,
  "queryKey" | "queryFn" | "select"
> &
  DsqlVariables<Operation> &
  DsqlContextScopeOptions<Operation> & {
    readonly select?: (
      result: DsqlOperationResult<Operation>,
    ) => Selected;
  };

export type DsqlExecuteOptions<Operation extends AnyDsqlOperation> =
  DsqlVariables<Operation>;

type DsqlServerFunction<Operation extends AnyDsqlOperation> = (options: {
  readonly data: DsqlServerVariables<Operation>;
}) => Promise<DsqlOperationWireResult<Operation>>;

export function queryKey<Operation extends AnyDsqlOperation>(
  operation: Operation,
  variables: DsqlVariables<Operation>,
  contextScope?: string,
): DsqlQueryKey<DsqlWireVariables<Operation>> {
  return dsqlQueryKey(operation, variables, contextScope);
}

export function queryOptions<
  Operation extends AnyDsqlOperation,
  Selected = DsqlOperationResult<Operation>,
>(
  operation: Operation,
  ...args: DsqlOptionsArgument<
    Operation,
    DsqlQueryOptions<Operation, Selected>
  >
) {
  const [options = {} as DsqlQueryOptions<Operation, Selected>] = args;
  const {
    contextScope,
    params = {},
    input = {},
    select,
    ...tanStackOptions
  } = options;
  const variables = { params, input } as DsqlVariables<Operation>;
  const wireVariables = materializeDsqlOperationVariables(
    operation,
    variables,
  );
  const serverFn = serverFunctionFor(operation);
  return tanStackQueryOptions({
    ...tanStackOptions,
    queryKey: dsqlQueryKeyForWire(
      operation,
      wireVariables,
      contextScope,
    ),
    queryFn: () => serverFn({ data: wireVariables }),
    select: dsqlResultSelector(operation, select),
  });
}

export async function executeQuery<Operation extends AnyDsqlOperation>(
  operation: Operation,
  ...args: DsqlOptionsArgument<
    Operation,
    DsqlExecuteOptions<Operation>
  >
): Promise<DsqlOperationResult<Operation>> {
  const [options = {} as DsqlExecuteOptions<Operation>] = args;
  const {
    params = {},
    input = {},
  } = options;
  const variables = { params, input } as DsqlVariables<Operation>;
  const wireVariables = materializeDsqlOperationVariables(
    operation,
    variables,
  );
  const serverFn = serverFunctionFor(operation);
  const wireResult = await serverFn({ data: wireVariables });
  return parseDsqlResult(operation, wireResult);
}

export function useQuery<
  Operation extends AnyDsqlOperation,
  Selected = DsqlOperationResult<Operation>,
>(
  operation: Operation,
  ...args: DsqlOptionsArgument<
    Operation,
    DsqlQueryOptions<Operation, Selected>
  >
): UseQueryResult<Selected, Error> {
  return useTanStackQuery(
    queryOptions<Operation, Selected>(operation, ...args),
  ) as UseQueryResult<Selected, Error>;
}

function serverFunctionFor<Operation extends AnyDsqlOperation>(
  operation: Operation,
): DsqlServerFunction<Operation> {
  const serverFn = serverFunctions[operation.name];
  if (!serverFn) {
    throw new Error(`missing dsql server function for ${operation.name}`);
  }

  return serverFn as DsqlServerFunction<Operation>;
}
