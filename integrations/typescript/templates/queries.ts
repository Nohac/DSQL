export type DsqlOperation<Result = unknown> = {
  readonly name: string;
  readonly kind: "query";
  readonly sql: string;
  readonly result?: Result;
};
