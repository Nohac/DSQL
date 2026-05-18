export type DsqlOperation<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
> = {
  readonly id: string;
  readonly name: string;
  readonly kind: "query";
  readonly sql: string;
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
};

export type DsqlOperationResult<Operation> =
  Operation extends DsqlOperation<infer Result> ? Result : never;

export type DsqlOperationParams<Operation> =
  Operation extends DsqlOperation<unknown, infer Params, unknown>
    ? Params
    : Record<string, never>;

export type DsqlOperationInput<Operation> =
  Operation extends DsqlOperation<unknown, unknown, infer Input>
    ? Input
    : Record<string, never>;

