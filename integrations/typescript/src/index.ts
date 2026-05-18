export type {
  BuildManifest,
  DynamicInputField,
  DynamicInputMetadata,
  HandoffMetadata,
  InputField,
  OperationMetadata,
  OperationManifestEntry,
  PolicyMetadata,
  ProvidedContextMetadata,
  ResultField,
  ResultShape,
  SourceMapEntry,
  SourceRange,
  SqlMetadata,
  SqlVariantMetadata,
} from "./generated/metadata.js";

import type { OperationMetadata } from "./generated/metadata.js";

export type DsqlQueryDefinition<
  TParams = unknown,
  TContext = unknown,
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
  TContext = unknown,
  TResult = unknown,
> = DsqlQueryDefinition<TParams, TContext, TResult> & {
  key(params: TParams): readonly ["dsql", string, TParams];
  queryOptions(params: TParams): {
    readonly queryKey: readonly ["dsql", string, TParams];
  };
};

export function defineDsqlQuery<
  TParams = unknown,
  TContext = unknown,
  TResult = unknown,
>(
  definition: DsqlQueryDefinition<TParams, TContext, TResult>,
): DsqlQuery<TParams, TContext, TResult> {
  return {
    ...definition,
    key(params) {
      return ["dsql", definition.name, params] as const;
    },
    queryOptions(params) {
      return {
        queryKey: ["dsql", definition.name, params] as const,
      };
    },
  };
}
