export type DsqlOperation<
  Result = unknown,
  Params = Record<string, never>,
  Input = Record<string, never>,
> = {
  readonly id: string;
  readonly name: string;
  readonly kind: "query";
  readonly result?: Result;
  readonly params?: Params;
  readonly input?: Input;
};

export type DsqlOperationResult<Operation> =
  Operation extends DsqlOperation<infer Result, any, any> ? Result : never;

export type DsqlOperationParams<Operation> =
  Operation extends DsqlOperation<any, infer Params, any>
    ? Params
    : Record<string, never>;

export type DsqlOperationInput<Operation> =
  Operation extends DsqlOperation<any, any, infer Input>
    ? Input
    : Record<string, never>;

export type DsqlFragmentDefinition<Result = unknown> = {
  readonly name: string;
  readonly kind: "fragment";
  readonly table: string;
  readonly result?: Result;
};

export type DsqlFragment<Fragment> =
  Fragment extends DsqlFragmentDefinition<infer Result> ? Result : never;
