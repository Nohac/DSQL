export type DsqlOperation<Result = unknown> = {
  readonly name: string;
  readonly kind: "query";
  readonly sql: string;
  readonly result?: Result;
};

export function dsql(strings: TemplateStringsArray, ...values: never[]): string {
  if (values.length > 0) {
    throw new Error("dsql templates do not support JavaScript interpolation");
  }

  return strings.raw[0] ?? "";
}
