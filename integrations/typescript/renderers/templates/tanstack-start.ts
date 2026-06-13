// @ts-nocheck
import {
  createServerFn,
  createServerOnlyFn,
  getGlobalStartContext,
} from "@tanstack/react-start";
import type {
  DsqlOperation,
  DsqlOperationResult,
  DsqlVariables,
} from "@dsql/typescript/runtime";
import { materializeDsqlQuery } from "@dsql/typescript/runtime";

export type DsqlServerVariables<
  Operation extends DsqlOperation<any, any, any>,
> = DsqlVariables<Operation>;

export type DsqlServerOperation<
  Operation extends DsqlOperation<any, any, any>,
> = Operation & {
  readonly sql: string;
  readonly parameters: readonly DsqlSqlParameter[];
  readonly variants: DsqlSqlVariants;
};

export type DsqlSqlParameter = {
  readonly path: string;
};

export type DsqlSqlVariants = Record<string, Record<string, string>>;

export type DsqlExecutionRequest<
  Operation extends DsqlOperation<any, any, any>,
> = {
  readonly operation: DsqlServerOperation<Operation>;
  readonly variables: DsqlVariables<Operation>;
  readonly sql: string;
  readonly values: any[];
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

  const serverOperation = await serverOperationFor(operation);
  const materialized = materializeDsqlQuery(
    {
      operation: serverOperation,
      sql: serverOperation.sql,
      parameters: serverOperation.parameters,
      variants: serverOperation.variants,
    },
    variables,
  );
  return executeQuery({
    operation: serverOperation,
    variables,
    ...materialized,
  });
}

const serverOperationFor = createServerOnlyFn(
  async <Operation extends DsqlOperation<any, any, any>>(
    operation: Operation,
  ): Promise<DsqlServerOperation<Operation>> => {
    const { serverOperations } = await import("./tanstack-start.server");
    const serverOperation = serverOperations[operation.name];
    if (!serverOperation) {
      throw new Error(`missing dsql server operation for ${operation.name}`);
    }

    return serverOperation as DsqlServerOperation<Operation>;
  },
);
