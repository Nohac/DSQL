// @ts-nocheck
import { createServerFn } from "@tanstack/react-start";
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

export type DsqlServerRuntime = {
  readonly executeQuery: DsqlServerExecutor;
};

let runtime: DsqlServerRuntime | undefined;

export function configureDsqlServer(runtimeConfig: DsqlServerRuntime): void {
  runtime = runtimeConfig;
}

function executeDsqlOperation<Operation extends DsqlOperation<any, any, any>>(
  operation: Operation,
  variables: DsqlServerVariables<Operation>,
): Promise<DsqlOperationResult<Operation>> {
  if (!runtime) {
    throw new Error("dsql server runtime is not configured");
  }

  return runtime.executeQuery(operation, variables);
}
