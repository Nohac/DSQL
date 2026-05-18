import type {
  DsqlOperation,
  DsqlOperationInput,
  DsqlOperationParams,
  DsqlOperationResult,
} from "./operations";

export type DsqlServerVariables<Operation extends DsqlOperation<any, any, any>> = {
  readonly params: DsqlOperationParams<Operation>;
  readonly input: DsqlOperationInput<Operation>;
};

export type DsqlServerExecutor = <
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: Operation,
  variables: DsqlServerVariables<Operation>,
) => Promise<DsqlOperationResult<Operation>>;

export type TanStackStartServerFnFactory<ServerFn> = <
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: Operation,
  handler: (variables: DsqlServerVariables<Operation>) => Promise<DsqlOperationResult<Operation>>,
) => ServerFn;

export function createDsqlServerFunctionFactory<ServerFn>(
  createServerFunction: TanStackStartServerFnFactory<ServerFn>,
  execute: DsqlServerExecutor,
): TanStackStartServerFnFactory<ServerFn> {
  return (operation, handler = (variables) => execute(operation, variables)) =>
    createServerFunction(operation, handler);
}
