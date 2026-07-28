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
  DsqlVariables,
  DsqlWireVariables,
} from "@dsql/typescript/runtime";
import {
  dsqlQueryKey,
  dsqlQueryKeyForWire,
  materializeDsqlOperationVariables,
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
 * The generated server function materializes configured host scalars before
 * returning. TanStack therefore stores the final host result in its cache and
 * dehydration payload, and direct cache APIs expose the same result type.
 */
export type DsqlQueryOptions<
  Operation extends AnyDsqlOperation,
  Selected = DsqlOperationResult<Operation>,
> = Omit<
  UseQueryOptions<
    DsqlOperationResult<Operation>,
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
}) => Promise<DsqlOperationResult<Operation>>;

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
    ...(select ? { select } : {}),
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
  return serverFn({ data: wireVariables });
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
