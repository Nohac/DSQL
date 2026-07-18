// @ts-nocheck
import {
  createServerFn,
  createServerOnlyFn,
  getGlobalStartContext,
} from "@tanstack/react-start";
import type {
  DsqlExecutionPayload,
  DsqlOperation,
  DsqlOperationContext,
  DsqlOperationResult,
  DsqlVariables,
} from "@dsql/typescript/runtime";
import { materializeDsqlQuery } from "@dsql/typescript/runtime";

export type DsqlServerVariables<
  Operation extends DsqlOperation<any, any, any, any>,
> = DsqlVariables<Operation>;

export type DsqlExecutionRequest<
  Operation extends DsqlOperation<any, any, any, any>,
> = {
  readonly operation: Operation;
  readonly variables: DsqlVariables<Operation>;
  readonly context: DsqlOperationContext<Operation>;
  readonly sql: string;
  readonly values: unknown[];
};

export type DsqlServerExecutor = <
  Operation extends DsqlOperation<any, any, any, any>,
>(
  request: DsqlExecutionRequest<Operation>,
) => Promise<DsqlOperationResult<Operation>>;

export type DsqlContextProvider = <
  Operation extends DsqlOperation<any, any, any, any>,
>(request: {
  readonly operation: Operation;
  readonly variables: DsqlVariables<Operation>;
}) => Promise<DsqlOperationContext<Operation>> | DsqlOperationContext<Operation>;

export type DsqlServerContext = {
  readonly dsql: {
    readonly executeQuery: DsqlServerExecutor;
    readonly provideContext?: DsqlContextProvider;
  };
};

async function executeDsqlOperation<Operation extends DsqlOperation<any, any, any, any>>(
  operation: Operation,
  variables: DsqlVariables<Operation>,
): Promise<DsqlOperationResult<Operation>> {
  const context = getGlobalStartContext() as Partial<DsqlServerContext> | undefined;
  const executeQuery = context?.dsql?.executeQuery;
  if (!executeQuery) {
    throw new Error("dsql executor is not configured in TanStack Start request context");
  }

  const payload = await executionPayloadFor(operation);
  const trustedContext = context?.dsql?.provideContext
    ? await context.dsql.provideContext({ operation, variables })
    // Required context paths make this empty fallback refuse during
    // materialization; it never turns missing server context into SQL NULL.
    : ({} as DsqlOperationContext<Operation>);
  const materialized = materializeDsqlQuery(
    payload,
    variables,
    trustedContext,
  );
  return executeQuery({
    operation,
    variables,
    context: trustedContext,
    sql: materialized.sql,
    values: [...materialized.values],
  });
}

const executionPayloadFor = createServerOnlyFn(
  async <Operation extends DsqlOperation<any, any, any, any>>(
    operation: Operation,
  ): Promise<DsqlExecutionPayload<Operation>> => {
    const { executionPayloads } = await import("./tanstack-start.server");
    const payload = executionPayloads[operation.name];
    if (!payload) {
      throw new Error(`missing dsql execution payload for ${operation.name}`);
    }

    return payload as DsqlExecutionPayload<Operation>;
  },
);
