// @ts-nocheck
import {
  createServerFn,
  createServerOnlyFn,
  getGlobalStartContext,
} from "@tanstack/react-start";
import type {
  DsqlOperation,
  DsqlOperationInput,
  DsqlOperationParams,
  DsqlOperationResult,
} from "./operations";

export type DsqlServerVariables<
  Operation extends DsqlOperation<any, any, any>,
> = {
  readonly params: DsqlOperationParams<Operation>;
  readonly input: DsqlOperationInput<Operation>;
};

export type DsqlServerOperation<
  Operation extends DsqlOperation<any, any, any>,
> = Operation & {
  readonly sql: string;
};

export type DsqlServerExecutor = <
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: DsqlServerOperation<Operation>,
  variables: DsqlServerVariables<Operation>,
) => Promise<DsqlOperationResult<Operation>>;

export type DsqlServerContext = {
  readonly dsql: {
    readonly executeQuery: DsqlServerExecutor;
  };
};

async function executeDsqlOperation<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  variables: DsqlServerVariables<Operation>,
): Promise<DsqlOperationResult<Operation>> {
  const context = getGlobalStartContext() as Partial<DsqlServerContext> | undefined;
  const executeQuery = context?.dsql?.executeQuery;
  if (!executeQuery) {
    throw new Error("dsql executor is not configured in TanStack Start request context");
  }

  return executeQuery(await serverOperationFor(operation), variables);
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
