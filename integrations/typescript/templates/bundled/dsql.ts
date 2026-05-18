import type { DsqlOperation } from "./operations";

export type DsqlOperationForSource<Source extends string> =
  Source extends keyof DsqlOperationBySource
    ? DsqlOperationBySource[Source]
    : DsqlOperation;

export function dsql<const Source extends string>(
  source: Source,
): DsqlOperationForSource<Source>;
export function dsql(strings: TemplateStringsArray, ...values: never[]): DsqlOperation;
export function dsql(
  input: string | TemplateStringsArray,
  ...values: never[]
): DsqlOperation {
  if (values.length > 0) {
    throw new Error("dsql templates do not support JavaScript interpolation");
  }

  const source = typeof input === "string" ? input : (input.raw[0] ?? "");
  const name = /\bquery\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source)?.[1];
  return {
    id: "<untransformed>",
    name: name ?? "<untransformed>",
    kind: "query",
    sql: "",
  };
}

