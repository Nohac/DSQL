// @ts-nocheck
import {
  createServerFn,
  createServerOnlyFn,
  getGlobalStartContext,
} from "@tanstack/react-start";
import type {
  DsqlExecutionPayload,
  DsqlOperation,
  DsqlOperationResult,
  DsqlVariables,
} from "@dsql/typescript/runtime";
import { materializeDsqlQuery } from "@dsql/typescript/runtime";

export type DsqlServerVariables<
  Operation extends DsqlOperation<any, any, any>,
> = DsqlVariables<Operation>;

export type DsqlExecutionRequest<
  Operation extends DsqlOperation<any, any, any>,
> = {
  readonly operation: Operation;
  readonly variables: DsqlVariables<Operation>;
  readonly sql: string;
  readonly values: unknown[];
};

export type DsqlServerExecutor = <
  Operation extends DsqlOperation<any, any, any>,
>(
  request: DsqlExecutionRequest<Operation>,
) => Promise<DsqlOperationResult<Operation>>;

export type DsqlServerContext = {
  readonly dsql: {
    readonly executeQuery: DsqlServerExecutor;
  };
};

async function executeDsqlOperation<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  variables: DsqlVariables<Operation>,
): Promise<DsqlOperationResult<Operation>> {
  const context = getGlobalStartContext() as Partial<DsqlServerContext> | undefined;
  const executeQuery = context?.dsql?.executeQuery;
  if (!executeQuery) {
    throw new Error("dsql executor is not configured in TanStack Start request context");
  }

  const payload = await executionPayloadFor(operation);
  const materialized = materializeDsqlQuery(
    payload,
    variables,
  );
  return executeQuery({
    operation,
    variables,
    sql: materialized.sql,
    values: [...materialized.values],
  });
}

const executionPayloadFor = createServerOnlyFn(
  async <Operation extends DsqlOperation<any, any, any>>(
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
