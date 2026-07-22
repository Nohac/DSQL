export type {
  BuildManifest,
  DynamicInputField,
  DynamicInputMetadata,
  FragmentManifestEntry,
  FragmentMetadata,
  FragmentSpreadMetadata,
  HandoffMetadata,
  InputField,
  OperationManifestEntry,
  OperationMetadata,
  PolicyApplicationMetadata,
  PolicyFieldAccessMetadata,
  PolicyMetadata,
  ProvidedContextMetadata,
  ResultField,
  ResultShape,
  SourceMapEntry,
  SourceRange,
  SqlMetadata,
  SqlVariantMetadata,
} from "./generated/metadata.ts";

export * from "./runtime.ts";

import type { OperationMetadata } from "./generated/metadata.ts";
import { getDsqlPath, materializeDsqlBindings } from "./runtime.ts";

export type DsqlQueryDefinition<
  TParams = unknown,
  TContext = Record<string, never>,
  TResult = unknown,
> = OperationMetadata & {
  readonly types?: {
    readonly params?: TParams;
    readonly context?: TContext;
    readonly result?: TResult;
  };
};

export type DsqlQuery<
  TParams = unknown,
  TContext = Record<string, never>,
  TResult = unknown,
> = DsqlQueryDefinition<TParams, TContext, TResult> & {
  key(
    params: TParams,
    ...contextScope: DsqlContextScopeArgument<TContext>
  ): readonly ["dsql", string, string | null, TParams];
  queryOptions(
    params: TParams,
    ...contextScope: DsqlContextScopeArgument<TContext>
  ): {
    readonly queryKey: readonly ["dsql", string, string | null, TParams];
  };
};

type DsqlContextScopeArgument<Context> =
  [Context] extends [Record<string, never>]
    ? readonly [contextScope?: string]
    : readonly [contextScope: string];

export function defineDsqlQuery<
  TParams = unknown,
  TContext = Record<string, never>,
  TResult = unknown,
>(
  definition: DsqlQueryDefinition<TParams, TContext, TResult>,
): DsqlQuery<TParams, TContext, TResult> {
  return {
    ...definition,
    key(params, ...contextScope) {
      const [scope] = contextScope;
      requireContextScope(definition, scope);
      return [
        "dsql",
        definition.name,
        scope ?? null,
        materializeParams(definition, params),
      ] as const;
    },
    queryOptions(params, ...contextScope) {
      const [scope] = contextScope;
      requireContextScope(definition, scope);
      return {
        queryKey: [
          "dsql",
          definition.name,
          scope ?? null,
          materializeParams(definition, params),
        ] as const,
      };
    },
  };
}

function materializeParams<TParams>(
  definition: OperationMetadata,
  params: TParams,
): TParams {
  const bindings = materializeDsqlBindings(definition.params, { params });
  return getDsqlPath(bindings, "params") as TParams;
}

function requireContextScope(
  definition: OperationMetadata,
  contextScope: string | undefined,
): void {
  if (definition.context.length > 0 && contextScope === undefined) {
    throw new Error(
      `dsql operation ${definition.name} requires contextScope for cache identity`,
    );
  }
}
