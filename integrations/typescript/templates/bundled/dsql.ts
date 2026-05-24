import type { DsqlFragmentDefinition, DsqlOperation } from "./operations";

export type DsqlDefinition = DsqlOperation | DsqlFragmentDefinition;

export type DsqlDefinitionForSource<Source extends string> =
  Source extends keyof DsqlDefinitionBySource
    ? DsqlDefinitionBySource[Source]
    : DsqlDefinition;

export function dsql<const Source extends string>(
  source: Source,
): DsqlDefinitionForSource<Source>;
export function dsql(
  strings: TemplateStringsArray,
  ...values: never[]
): DsqlDefinition;
export function dsql(
  input: string | TemplateStringsArray,
  ...values: never[]
): DsqlDefinition {
  if (values.length > 0) {
    throw new Error("dsql templates do not support JavaScript interpolation");
  }

  const source = typeof input === "string" ? input : (input.raw[0] ?? "");
  const queryName = /\bquery\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source)?.[1];
  if (queryName) {
    return {
      id: "<untransformed>",
      name: queryName,
      kind: "query",
    };
  }
  const fragment = /\bfragment\s+([A-Za-z_][A-Za-z0-9_]*)\s+on\s+([A-Za-z_][A-Za-z0-9_$.]*)\b/.exec(
    source,
  );
  if (fragment) {
    return {
      name: fragment[1] ?? "<untransformed>",
      kind: "fragment",
      table: fragment[2] ?? "<untransformed>",
    };
  }
  return {
    id: "<untransformed>",
    name: "<untransformed>",
    kind: "query",
  };
}
