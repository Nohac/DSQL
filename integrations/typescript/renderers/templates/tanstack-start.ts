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
  readonly variables: DsqlServerVariables<Operation>;
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
  variables: DsqlServerVariables<Operation>,
): Promise<DsqlOperationResult<Operation>> {
  const context = getGlobalStartContext() as Partial<DsqlServerContext> | undefined;
  const executeQuery = context?.dsql?.executeQuery;
  if (!executeQuery) {
    throw new Error("dsql executor is not configured in TanStack Start request context");
  }

  const serverOperation = await serverOperationFor(operation);
  const materialized = materializeDsqlOperation(serverOperation, variables);
  return executeQuery({
    operation: serverOperation,
    variables,
    ...materialized,
  });
}

function materializeDsqlOperation<
  Operation extends DsqlOperation<any, any, any>,
>(
  operation: DsqlServerOperation<Operation>,
  variables: DsqlServerVariables<Operation>,
): { readonly sql: string; readonly values: any[] } {
  let sql = operation.sql;
  for (const [path, cases] of Object.entries(operation.variants)) {
    const selected = getPath(variables, path);
    if (typeof selected !== "string") {
      throw new Error(`missing dsql variant value at ${path}`);
    }
    const replacement = cases[selected];
    if (replacement === undefined) {
      throw new Error(`invalid dsql variant value at ${path}: ${selected}`);
    }
    sql = sql.replaceAll(`{{${path}}}`, replacement);
  }

  return {
    sql,
    values: operation.parameters.map((parameter) =>
      getPath(variables, parameter.path),
    ),
  };
}

function getPath(value: unknown, path: string): unknown {
  let current = value;
  for (const part of path.split(".")) {
    if (current === null || typeof current !== "object") {
      return undefined;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return current;
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
