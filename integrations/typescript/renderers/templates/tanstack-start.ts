// @ts-nocheck
import { createServerFn, getGlobalStartContext } from "@tanstack/react-start";
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

export type DsqlServerExecutor = <
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: Operation,
  variables: DsqlServerVariables<Operation>,
) => Promise<DsqlOperationResult<Operation>>;

export type DsqlServerContext = {
  readonly dsql: {
    readonly executeQuery: DsqlServerExecutor;
  };
};

function executeDsqlOperation<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  variables: DsqlServerVariables<Operation>,
): Promise<DsqlOperationResult<Operation>> {
  const context = getGlobalStartContext() as Partial<DsqlServerContext> | undefined;
  const executeQuery = context?.dsql?.executeQuery;
  if (!executeQuery) {
    throw new Error("dsql executor is not configured in TanStack Start request context");
  }

  return executeQuery(operation, variables);
}
