import {
  dsql as runtimeDsql,
  type DsqlDefinition,
} from "@dsql/typescript/runtime";

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
  return runtimeDsql(input as string, ...values);
}
