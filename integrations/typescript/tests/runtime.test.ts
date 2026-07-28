import { expect, test } from "bun:test";
import defaultCases from "../../../tests/conformance/input-defaults.json" with {
  type: "json",
};
import dynamicCases from "../../../tests/conformance/dynamic-inputs.json" with {
  type: "json",
};
import inputCases from "../../../tests/conformance/input-values.json" with {
  type: "json",
};
import {
  applyDsqlVariants,
  assignDsqlResultField,
  collectDsqlParameterValues,
  dsqlQueryKey,
  getDsqlPath,
  materializeDsqlDatabaseArrayResult,
  materializeDsqlBindings,
  materializeDsqlExecutionResult,
  materializeDsqlOperationVariables,
  materializeDsqlQuery,
  materializeDsqlScalarResult,
  type DsqlDatabaseArray,
  type DsqlExecutionPayload,
  type DsqlOperation,
  type DsqlWireContract,
} from "../src/runtime";

const emptyOperationContracts = {
  dynamicInputContracts: [],
} as const;

type DynamicValueKind = "scalar" | "collection" | "boolean";

function dynamicOperatorContracts(
  input: {
    readonly sites: readonly {
      readonly fields: readonly {
        readonly key: string;
        readonly operators: readonly {
          readonly name: string;
          readonly value_kind: string;
        }[];
      }[];
    }[];
  },
  fieldKey: string,
  names: readonly string[],
): Array<{ readonly name: string; readonly value_kind: DynamicValueKind }> {
  return names.map((name) => {
    const kinds = new Set(
      input.sites.flatMap((site) =>
        site.fields
          .filter((field) => field.key === fieldKey)
          .flatMap((field) =>
            field.operators
              .filter((operator) => operator.name === name)
              .map((operator) => operator.value_kind),
          ),
      ),
    );
    const [kind] = [...kinds];
    if (
      kinds.size !== 1 ||
      (kind !== "scalar" && kind !== "collection" && kind !== "boolean")
    ) {
      throw new Error(
        `test metadata has no unique value kind for ${fieldKey}.${name}`,
      );
    }
    return { name, value_kind: kind };
  });
}

const movieInputs = [
  {
    path: "input.movie_info.clause.where.id.op",
    data_type: "text",
    wire: { encoding: "text" },
    validation: {},
    required: true,
    nullable: false,
  },
  {
    path: "input.movie_info.clause.where.id.value",
    data_type: "int",
    wire: { encoding: "integer" },
    validation: {},
    required: true,
    nullable: false,
  },
] as const;

type MovieOperation = DsqlOperation<
  { readonly id: number },
  Record<string, never>,
  {
    readonly movie_info: {
      readonly clause: {
        readonly where: {
          readonly id: {
            readonly op: "eq" | "ne";
            readonly value: number;
          };
        };
      };
    };
  },
  { readonly tenant_id: string }
>;

const MovieInfoOperation = {
  id: "movie-info-hash",
  name: "MovieInfo",
  kind: "query",
  requiresContext: true,
  inputs: movieInputs,
  ...emptyOperationContracts,
} satisfies MovieOperation;

const payload = {
  operation: MovieInfoOperation,
  sql: "select * from movie_info where id {{input.movie_info.clause.where.id.op}} $1 and tenant_id = $2",
  parameters: [
    { path: "input.movie_info.clause.where.id.value" },
    { path: "context.tenant_id" },
  ],
  variants: {
    "input.movie_info.clause.where.id.op": {
      cases: {
        eq: "=",
        ne: "!=",
      },
    },
  },
  dynamicInputs: [],
  contextInputs: [
    {
      path: "context.tenant_id",
      data_type: "uuid",
      wire: { encoding: "uuid" },
      validation: {},
      required: true,
      nullable: false,
    },
  ],
} satisfies DsqlExecutionPayload<MovieOperation>;

const variables = {
  params: {},
  input: {
    movie_info: {
      clause: {
        where: {
          id: {
            op: "eq",
            value: 42,
          },
        },
      },
    },
  },
} as const;

test("materializes sql variants and ordered parameter values", () => {
  expect(
    materializeDsqlQuery(payload, variables, {
      tenant_id: "018f6f19-795f-7c3d-b1b3-8f177ab8a322",
    }),
  ).toEqual({
    sql: "select * from movie_info where id = $1 and tenant_id = $2",
    values: [42, "018f6f19-795f-7c3d-b1b3-8f177ab8a322"],
    context: {
      tenant_id: "018f6f19-795f-7c3d-b1b3-8f177ab8a322",
    },
  });
});

type OptionalOperation = DsqlOperation<
  { readonly id: number },
  { readonly direction?: "asc" | "desc" | null; readonly limit?: number | null },
  Record<string, never>,
  Record<string, never>
>;

const optionalInputs = [
    {
      path: "params.direction",
      data_type: "text",
      wire: { encoding: "text" },
      validation: {},
      required: false,
      nullable: true,
      default: { kind: "null" },
    },
    {
      path: "params.limit",
      data_type: "int",
      wire: { encoding: "integer" },
      validation: {},
      required: false,
      nullable: true,
      default: { kind: "number", value: "10" },
    },
] as const;

const OptionalOperation = {
  id: "optional-hash",
  name: "Optional",
  kind: "query",
  requiresContext: false,
  inputs: optionalInputs,
  ...emptyOperationContracts,
} satisfies OptionalOperation;

const optionalPayload = {
  operation: OptionalOperation,
  sql: "select * from movie_info order by case when '{{params.direction}}' = 'asc' then id end asc limit $1",
  parameters: [{ path: "params.limit" }],
  variants: {
    "params.direction": {
      cases: { asc: "asc", desc: "desc" },
      nullText: "null",
    },
  },
  dynamicInputs: [],
  contextInputs: [],
} satisfies DsqlExecutionPayload<OptionalOperation>;

test("materializes defaults and nullable sql variants without mutating inputs", () => {
  const variables = {};
  expect(materializeDsqlQuery(optionalPayload, variables, {})).toEqual({
    sql: "select * from movie_info order by case when 'null' = 'asc' then id end asc limit $1",
    values: [10],
    context: {},
  });
  expect(variables).toEqual({});

  expect(
    materializeDsqlQuery(optionalPayload, { params: { limit: null } }, {}),
  ).toEqual({
    sql: "select * from movie_info order by case when 'null' = 'asc' then id end asc limit $1",
    values: [null],
    context: {},
  });
});

test("materializes bounded dynamic inputs with shared operand positions", () => {
  const dynamicInputs = [
    {
      path: "params.search",
      kind: "predicate",
      surface: "selected",
      fields: [
        {
          key: "id",
          catalog_path: "public.users.id",
          data_type: "int",
          wire: { encoding: "integer" },
          validation: {},
          nullable: false,
          access: "unconditional",
          operators: ["in"],
          directions: [],
        },
        {
          key: "name",
          catalog_path: "public.users.name",
          data_type: "text",
          wire: { encoding: "text" },
          validation: {},
          nullable: false,
          access: "unconditional",
          operators: ["like"],
          directions: [],
        },
      ],
      sites: ["u", "u2"].map((alias, index) => ({
        marker: `{{dynamic:${index}}}`,
        identity_sql: "TRUE",
        fields: [
          {
            key: "id",
            operators: [
              {
                name: "in",
                value_kind: "collection",
                before_value: `${alias}.id = ANY(`,
                after_value: ")",
                cases: [],
              },
            ],
            directions: [],
          },
          {
            key: "name",
            operators: [
              {
                name: "like",
                value_kind: "scalar",
                before_value: `${alias}.name LIKE `,
                cases: [],
              },
            ],
            directions: [],
          },
        ],
      })),
    },
    {
      path: "params.order",
      kind: "order",
      surface: "selected",
      fields: [
        {
          key: "name",
          catalog_path: "public.users.name",
          data_type: "text",
          wire: { encoding: "text" },
          validation: {},
          nullable: false,
          access: "unconditional",
          operators: [],
          directions: ["desc_nulls_last"],
        },
      ],
      sites: [
        {
          marker: "{{dynamic:2}}",
          identity_sql: "NULL::integer",
          fields: [
            {
              key: "name",
              operators: [],
              directions: [
                {
                  value: "desc_nulls_last",
                  text: "u.name DESC NULLS LAST",
                },
              ],
            },
          ],
        },
      ],
    },
  ];
  const operation = {
    id: "dynamic",
    name: "Dynamic",
    kind: "query",
    requiresContext: false,
    inputs: [
      {
        path: "params.tenant",
        data_type: "int",
        wire: { encoding: "integer" },
        validation: {},
        required: true,
        nullable: false,
      },
      {
        path: "params.search",
        data_type: "dynamic_predicate",
        wire: { encoding: "unsupported" },
        validation: {},
        required: true,
        nullable: false,
      },
      {
        path: "params.order",
        data_type: "dynamic_order",
        wire: { encoding: "unsupported" },
        validation: {},
        required: true,
        nullable: false,
      },
    ],
    dynamicInputContracts: dynamicInputs.map((input) => ({
      path: input.path,
      kind: input.kind,
      fields: input.fields.map((field) => ({
        key: field.key,
        data_type: field.data_type,
        wire: field.wire,
        validation: field.validation,
        operators: dynamicOperatorContracts(
          input,
          field.key,
          field.operators,
        ),
      })),
    })),
  } satisfies DsqlOperation;
  const dynamicPayload = {
    operation,
    sql: "select $1 where {{dynamic:0}} and {{dynamic:1}} order by {{dynamic:2}}",
    parameters: [{ path: "params.tenant" }],
    variants: {},
    dynamicInputs,
    contextInputs: [],
  } satisfies DsqlExecutionPayload;

  expect(
    materializeDsqlQuery(
      dynamicPayload,
      {
        params: {
          tenant: 7,
          search: {
            and: [{ name: { like: "A%" } }, {}],
            or: [{ id: { in: [1, 2] } }],
          },
          order: [{ name: "desc_nulls_last" }],
        },
      },
      {},
    ),
  ).toEqual({
    sql: "select $1 where (((u.name LIKE $2)) AND ((u.id = ANY($3)))) and (((u2.name LIKE $2)) AND ((u2.id = ANY($3)))) order by u.name DESC NULLS LAST",
    values: [7, "A%", [1, 2]],
    context: {},
  });

  expect(() =>
    materializeDsqlQuery(
      {
        ...dynamicPayload,
        operation: {
          ...operation,
          dynamicInputContracts: operation.dynamicInputContracts.filter(
            (input) => input.path !== "params.search",
          ),
        },
      },
      {
        params: {
          tenant: 7,
          search: { name: { like: "A%" } },
          order: [],
        },
      },
      {},
    ),
  ).toThrow(
    "invalid dsql dynamic metadata: input params.search has no client materialization contract",
  );

  const replaceIdType = <Field extends { readonly key: string }>(
    field: Field,
    dataType: string,
  ) =>
    field.key === "id"
      ? {
          ...field,
          data_type: dataType,
          wire: {
            encoding: dataType === "int" ? "integer" : dataType,
          },
        }
      : field;
  const withIdDataType = (dataType: string) => ({
    ...dynamicPayload,
    operation: {
      ...operation,
      dynamicInputContracts: operation.dynamicInputContracts.map((input) =>
        input.path === "params.search"
          ? {
              ...input,
              fields: input.fields.map((field) =>
                replaceIdType(field, dataType),
              ),
            }
          : input,
      ),
    },
    dynamicInputs: dynamicPayload.dynamicInputs.map((input) =>
      input.path === "params.search"
        ? {
            ...input,
            fields: input.fields.map((field) =>
              replaceIdType(field, dataType),
            ),
          }
        : input,
    ),
  });
  const materializeId = (dataType: string, value: unknown) =>
    materializeDsqlQuery(
      withIdDataType(dataType),
      {
        params: {
          tenant: 7,
          search: { id: { in: [value] } },
          order: [],
        },
      },
      {},
    );

  const uuid = "00000000-0000-0000-0000-000000000000";
  expect(materializeId("uuid", uuid).values).toEqual([7, [uuid]]);
  expect(() => materializeId("uuid", "not-a-uuid")).toThrow("valid uuid");
  const timestamp = "2024-02-29T12:34:56.789+01:30";
  expect(materializeId("timestamptz", timestamp).values).toEqual([
    7,
    [timestamp],
  ]);
  expect(() =>
    materializeId("timestamptz", "2025-02-29T12:34:56Z"),
  ).toThrow("valid timestamptz");
  expect(materializeId("numeric", "123.45e-2").values).toEqual([
    7,
    ["123.45e-2"],
  ]);
  expect(() => materializeId("numeric", "12.3.4")).toThrow("valid numeric");

  const dateWire = {
    encoding: "text_cast",
    provider_type: {
      schema: "pg_catalog",
      name: "date",
    },
  } as const;
  const textCastBase = withIdDataType("text");
  const textCastPayload = {
    ...textCastBase,
    operation: {
      ...textCastBase.operation,
      dynamicInputContracts: textCastBase.operation.dynamicInputContracts.map(
        (input) =>
          input.path === "params.search"
            ? {
                ...input,
                fields: input.fields.map((field) =>
                  field.key === "id" ? { ...field, wire: dateWire } : field,
                ),
              }
            : input,
      ),
    },
    dynamicInputs: textCastBase.dynamicInputs.map((input) =>
      input.path === "params.search"
        ? {
            ...input,
            fields: input.fields.map((field) =>
              field.key === "id"
                ? {
                    ...field,
                    wire: dateWire,
                  }
                : field,
            ),
          }
        : input,
    ),
  };
  const materializeDate = (value: unknown) =>
    materializeDsqlQuery(
      textCastPayload,
      {
        params: {
          tenant: 7,
          search: { id: { in: [value] } },
          order: [],
        },
      },
      {},
    );
  expect(materializeDate("2026-07-27").values).toEqual([
    7,
    ["2026-07-27"],
  ]);
  expect(() => materializeDate(20260727)).toThrow("valid text");
});

test("matches the shared typed-default conformance cases", () => {
  for (const testCase of defaultCases) {
    const materialize = () =>
      materializeDsqlBindings([testCase.field], {}, "wire");
    if ("expected" in testCase) {
      expect(materialize(), testCase.name).toEqual({
        params: { value: testCase.expected },
      });
    } else {
      expect(materialize, testCase.name).toThrow(testCase.error);
    }
  }
});

test("matches the shared supplied-value conformance cases", () => {
  for (const testCase of inputCases) {
    const materialize = () =>
      materializeDsqlBindings(
        [testCase.field],
        testCase.bindings,
        "wire",
      );
    if ("expected" in testCase) {
      const materialized = materialize();
      expect(
        getDsqlPath(materialized, testCase.field.path),
        testCase.name,
      ).toEqual(testCase.expected);
    } else {
      expect(materialize, testCase.name).toThrow(testCase.error);
    }
  }
});

test("matches the shared bounded dynamic input conformance cases", () => {
  const operation: DsqlOperation<unknown, Record<string, unknown>> = {
    id: "dynamic-conformance",
    name: dynamicCases.operation.name,
    kind: "query",
    requiresContext: false,
    inputs: dynamicCases.operation.params,
    dynamicInputContracts: dynamicCases.operation.dynamic_inputs.map(
      (input) => ({
        path: input.path,
        kind: input.kind,
        fields: input.fields.map((field) => ({
          key: field.key,
          data_type: field.data_type,
          wire: field.wire,
          validation: field.validation,
          operators: dynamicOperatorContracts(
            input,
            field.key,
            field.operators,
          ),
        })),
      }),
    ),
  };
  const payload: DsqlExecutionPayload<typeof operation> = {
    operation,
    sql: dynamicCases.operation.sql.text,
    parameters: dynamicCases.operation.sql.parameters,
    variants: {},
    dynamicInputs: dynamicCases.operation.dynamic_inputs,
    contextInputs: [],
  };

  for (const testCase of dynamicCases.cases) {
    const materialize = () =>
      materializeDsqlQuery(payload, testCase.bindings, {});
    if ("expected_sql" in testCase) {
      expect(materialize(), testCase.name).toEqual({
        sql: testCase.expected_sql,
        values: testCase.expected_values,
        context: {},
      });
    } else {
      expect(materialize, testCase.name).toThrow(testCase.error);
    }
  }
});

test("materializes copy-on-write and preserves unrelated host values", () => {
  const date = new Date("2026-07-22T00:00:00Z");
  const bytes = new Uint8Array([2, 5, 8]);
  const map = new Map([["answer", 42]]);
  const set = new Set(["kept"]);
  const untouched = { date, bytes, map, set };
  const bindings = { params: {}, untouched };

  const materialized = materializeDsqlBindings(
    optionalInputs,
    bindings,
    "wire",
  ) as {
    readonly params: { readonly direction: null; readonly limit: number };
    readonly untouched: typeof untouched;
  };

  expect(materialized).not.toBe(bindings);
  expect(materialized.params).not.toBe(bindings.params);
  expect(materialized.untouched).toBe(untouched);
  expect(materialized.untouched.date).toBe(date);
  expect(materialized.untouched.bytes).toBe(bytes);
  expect(materialized.untouched.map).toBe(map);
  expect(materialized.untouched.set).toBe(set);
  expect(materializeDsqlBindings([], bindings, "wire")).toBe(bindings);
});

test("rejects invalid envelopes, unused required fields, and context defaults", () => {
  const defaulted = {
    path: "params.nested.value",
    data_type: "int",
    wire: { encoding: "integer" },
    validation: {},
    required: false,
    nullable: false,
    default: { kind: "number", value: "7" },
  } as const;
  for (const params of [null, 5]) {
    expect(() =>
      materializeDsqlBindings([defaulted], { params }, "wire"),
    ).toThrow(
      "invalid dsql input envelope at params.nested.value",
    );
  }
  expect(() =>
    materializeDsqlBindings(
      [
        {
          path: "params.unused",
          data_type: "int",
          wire: { encoding: "integer" },
          validation: {},
          required: true,
          nullable: false,
        },
      ],
      {},
      "wire",
    ),
  ).toThrow("missing dsql input at params.unused");
  const requiredNullable = {
    path: "params.info",
    data_type: "text",
    wire: { encoding: "text" },
    validation: {},
    required: true,
    nullable: true,
  } as const;
  expect(
    materializeDsqlBindings(
      [requiredNullable],
      { params: { info: null } },
      "wire",
    ),
  ).toEqual({ params: { info: null } });
  expect(() =>
    materializeDsqlBindings(
      [requiredNullable],
      { params: {} },
      "wire",
    ),
  ).toThrow("missing dsql input at params.info");
  expect(() =>
    materializeDsqlBindings(
      [
        {
          path: "context.tenant_id",
          data_type: "uuid",
          wire: { encoding: "uuid" },
          validation: {},
          required: false,
          nullable: false,
          default: { kind: "string", value: "forged-default" },
        },
      ],
      { context: {} },
      "wire",
    ),
  ).toThrow("missing trusted dsql context at context.tenant_id");
});

test("cache keys materialize defaults and canonical dynamic identities", () => {
  const operation = {
    id: "search-hash",
    name: "Search",
    kind: "query",
    requiresContext: false,
    inputs: [
      {
        path: "params.limit",
        data_type: "int",
        wire: { encoding: "integer" },
        validation: {},
        required: false,
        nullable: false,
        default: { kind: "number", value: "10" },
      },
      {
        path: "params.search",
        data_type: "dynamic_predicate",
        wire: { encoding: "unsupported" },
        validation: {},
        required: false,
        nullable: true,
      },
      {
        path: "params.order",
        data_type: "dynamic_order",
        wire: { encoding: "unsupported" },
        validation: {},
        required: false,
        nullable: true,
      },
    ],
    dynamicInputContracts: [],
  } as const satisfies DsqlOperation;

  const omittedDefault = dsqlQueryKey(operation, {
    params: { search: null, order: null },
  });
  const explicitDefault = dsqlQueryKey(operation, {
    params: { limit: 10, search: {}, order: [] },
  });
  expect(omittedDefault).toEqual(explicitDefault);
  expect(omittedDefault[3]).toEqual({
    params: { limit: 10, search: {}, order: [] },
  });
});

test("serializes host inputs once before cache and server boundaries", () => {
  type DateValue = { readonly iso: string };
  type MappedOperation = DsqlOperation<
    Record<string, never>,
    {
      readonly since: DateValue;
      readonly dates: Array<DateValue | null>;
      readonly filter: {
        readonly released?: { readonly eq?: DateValue };
      };
    },
    Record<string, never>,
    { readonly cutoff: DateValue },
    DsqlWireContract<
      Record<string, never>,
      {
        readonly since: string;
        readonly dates: Array<string | null>;
        readonly filter: {
          readonly released?: { readonly eq?: string };
        };
      },
      Record<string, never>
    >
  >;
  const calls: string[] = [];
  const serialize = (value: DateValue): string => {
    calls.push(value.iso);
    return value.iso;
  };
  const operation = {
    id: "mapped-input",
    name: "MappedInput",
    kind: "query",
    requiresContext: true,
    inputs: [
      {
        path: "params.since",
        data_type: "date",
        wire: { encoding: "text_cast" },
        validation: {},
        required: true,
        nullable: false,
        serialize,
      },
      {
        path: "params.dates",
        data_type: "date",
        wire: { encoding: "text_cast" },
        validation: {},
        collection: true,
        required: true,
        nullable: false,
        serialize,
      },
      {
        path: "params.filter",
        data_type: "dynamic_predicate",
        wire: { encoding: "unsupported" },
        validation: {},
        required: true,
        nullable: false,
      },
    ],
    dynamicInputContracts: [
      {
        path: "params.filter",
        kind: "predicate",
        fields: [
          {
            key: "released",
            data_type: "date",
            wire: { encoding: "text_cast" },
            validation: {},
            operators: [{ name: "eq", value_kind: "scalar" }],
            serialize,
          },
        ],
      },
    ],
  } satisfies MappedOperation;
  const hostVariables = {
    params: {
      since: { iso: "2026-01-01" },
      dates: [{ iso: "2026-02-02" }, null],
      filter: { released: { eq: { iso: "2026-03-03" } } },
    },
  } as const;

  const wireVariables = materializeDsqlOperationVariables(
    operation,
    hostVariables,
  );
  expect(wireVariables).toEqual({
    params: {
      since: "2026-01-01",
      dates: ["2026-02-02", null],
      filter: { released: { eq: "2026-03-03" } },
    },
  });
  expect(calls).toEqual(["2026-01-01", "2026-02-02", "2026-03-03"]);
  expect(hostVariables.params.since).toEqual({ iso: "2026-01-01" });

  const materialized = materializeDsqlQuery(
    {
      operation,
      sql: "select $1, $2",
      parameters: [
        { path: "params.since" },
        { path: "context.cutoff" },
      ],
      variants: {},
      dynamicInputs: [],
      contextInputs: [
        {
          path: "context.cutoff",
          data_type: "date",
          wire: { encoding: "text_cast" },
          validation: {},
          required: true,
          nullable: false,
          serialize,
        },
      ],
    },
    wireVariables,
    { cutoff: { iso: "2026-04-04" } },
  );
  expect(materialized).toEqual({
    sql: "select $1, $2",
    values: ["2026-01-01", "2026-04-04"],
    context: { cutoff: "2026-04-04" },
  });
  expect(calls).toEqual([
    "2026-01-01",
    "2026-02-02",
    "2026-03-03",
    "2026-04-04",
  ]);
});

test("materializes executor-owned results in place at configured leaves", () => {
  type DateValue = { readonly iso: string };
  type HostResult = {
    readonly released: DateValue;
    readonly movies: Array<{
      readonly premiered: DateValue | null;
      readonly dates: DsqlDatabaseArray<DateValue>;
    }>;
  };
  type WireResult = {
    readonly released: string;
    readonly movies: Array<{
      readonly premiered: string | null;
      readonly dates: DsqlDatabaseArray<string>;
    }>;
  };
  let parseCalls = 0;
  const parse = (value: unknown): DateValue => {
    parseCalls += 1;
    if (typeof value !== "string") {
      throw new Error("date wire value must be a string");
    }
    return { iso: value };
  };
  const operation = {
    id: "mapped-result",
    name: "MappedResult",
    kind: "query",
    requiresContext: false,
    inputs: [],
    dynamicInputContracts: [],
  } satisfies DsqlOperation<
    HostResult,
    Record<string, never>,
    Record<string, never>,
    Record<string, never>,
    DsqlWireContract<
      WireResult,
      Record<string, never>,
      Record<string, never>
    >
  >;
  const executionPayload = {
    operation,
    sql: "select result",
    parameters: [],
    variants: {},
    dynamicInputs: [],
    contextInputs: [],
    materializeResult(result) {
      assignDsqlResultField(
        result,
        "released",
        materializeDsqlScalarResult(
          result.released,
          parse,
          "released",
          "date",
        ),
      );
      for (const movie of result.movies) {
        if (movie.premiered !== null) {
          assignDsqlResultField(
            movie,
            "premiered",
            materializeDsqlScalarResult(
              movie.premiered,
              parse,
              "movies.premiered",
              "date",
            ),
          );
        }
        assignDsqlResultField(
          movie,
          "dates",
          materializeDsqlDatabaseArrayResult(
            movie.dates,
            parse,
            "movies.dates",
            "date",
          ),
        );
      }
      return result as unknown as HostResult;
    },
  } satisfies DsqlExecutionPayload<typeof operation>;
  const wire: WireResult = {
    released: "2026-01-01",
    movies: [
      {
        premiered: null,
        dates: ["2026-02-02", ["2026-03-03", null]],
      },
    ],
  };
  const movies = wire.movies;
  const movie = movies[0];
  if (!movie) {
    throw new Error("test fixture is missing its movie");
  }
  const dates = movie.dates;
  const nestedDates = dates[1];
  if (!Array.isArray(nestedDates)) {
    throw new Error("test fixture is missing its nested date array");
  }

  const materialized = materializeDsqlExecutionResult(executionPayload, wire);
  expect(materialized).toBe(wire as unknown as HostResult);
  expect(materialized.movies).toBe(movies);
  expect(materialized.movies[0]).toBe(movie);
  expect(materialized.movies[0]?.dates).toBe(dates);
  expect(materialized.movies[0]?.dates[1]).toBe(nestedDates);
  expect(materialized).toEqual({
    released: { iso: "2026-01-01" },
    movies: [
      {
        premiered: null,
        dates: [
          { iso: "2026-02-02" },
          [{ iso: "2026-03-03" }, null],
        ],
      },
    ],
  });
  expect(parseCalls).toBe(3);
});

test("trusts codec-free results after one root check", () => {
  const operation = {
    id: "plain-result",
    name: "PlainResult",
    kind: "query",
    requiresContext: false,
    inputs: [],
    dynamicInputContracts: [],
  } satisfies DsqlOperation<{ readonly value: string }>;
  const executionPayload = {
    operation,
    sql: "select result",
    parameters: [],
    variants: {},
    dynamicInputs: [],
    contextInputs: [],
  } satisfies DsqlExecutionPayload<typeof operation>;
  const result = { value: "unchanged" };

  expect(materializeDsqlExecutionResult(executionPayload, result)).toBe(result);
  expect(() =>
    materializeDsqlExecutionResult(
      executionPayload,
      undefined as unknown as { readonly value: string },
    ),
  ).toThrow("dsql result for PlainResult must be an object");
});

test("rejects parser failures without returning a partial result", () => {
  const cause = new Error("invalid date");
  const operation = {
    id: "failing-result",
    name: "FailingResult",
    kind: "query",
    requiresContext: false,
    inputs: [],
    dynamicInputContracts: [],
  } satisfies DsqlOperation<
    { readonly first: { readonly iso: string }; readonly second: never },
    Record<string, never>,
    Record<string, never>,
    Record<string, never>,
    DsqlWireContract<
      { readonly first: string; readonly second: string },
      Record<string, never>,
      Record<string, never>
    >
  >;
  const executionPayload = {
    operation,
    sql: "select result",
    parameters: [],
    variants: {},
    dynamicInputs: [],
    contextInputs: [],
    materializeResult(result) {
      assignDsqlResultField(result, "first", { iso: result.first });
      assignDsqlResultField(
        result,
        "second",
        materializeDsqlScalarResult(
          result.second,
          () => {
            throw cause;
          },
          "second",
          "date",
        ),
      );
      return result as never;
    },
  } satisfies DsqlExecutionPayload<typeof operation>;
  const result = { first: "2026-01-01", second: "invalid" };

  try {
    materializeDsqlExecutionResult(executionPayload, result);
    throw new Error("expected result materialization to fail");
  } catch (error) {
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toBe(
      "dsql result parser failed at second (date)",
    );
    expect((error as Error).cause).toBe(cause);
  }
  expect(result.first).toEqual({ iso: "2026-01-01" });
});

test("requires trusted context separately from public variables", () => {
  const forgedVariables = {
    ...variables,
    context: { tenant_id: "attacker-controlled" },
  };

  expect(() =>
    materializeDsqlQuery(
      payload,
      forgedVariables,
      {} as { readonly tenant_id: string },
    ),
  ).toThrow("missing trusted dsql context at context.tenant_id");
});

test("requires an opaque context scope for context-dependent cache keys", () => {
  expect(() => dsqlQueryKey(MovieInfoOperation, variables)).toThrow(
    "dsql operation MovieInfo requires contextScope for cache identity",
  );
  expect(dsqlQueryKey(MovieInfoOperation, variables, "authorization-v7")).toEqual([
    "dsql",
    "MovieInfo",
    "authorization-v7",
    variables,
  ]);
  expect(
    dsqlQueryKey(
      { ...MovieInfoOperation, requiresContext: false },
      variables,
    ),
  ).toEqual(["dsql", "MovieInfo", null, variables]);
});

test("rejects null trusted context instead of binding SQL null", () => {
  expect(() =>
    materializeDsqlQuery(payload, variables, {
      tenant_id: null,
    } as unknown as { readonly tenant_id: string }),
  ).toThrow("missing trusted dsql context at context.tenant_id");
});

test("reads dotted paths from nested values", () => {
  expect(getDsqlPath(variables, "input.movie_info.clause.where.id.value")).toBe(
    42,
  );
  expect(getDsqlPath(variables, "input.movie_info.missing")).toBeUndefined();
  expect(getDsqlPath(variables, "")).toBe(variables);
});

test("collects missing parameter paths as undefined", () => {
  expect(
    collectDsqlParameterValues(
      [
        { path: "input.movie_info.clause.where.id.value" },
        { path: "input.movie_info.missing" },
      ],
      variables,
    ),
  ).toEqual([42, undefined]);
});

test("rejects missing dsql variant values", () => {
  expect(() =>
    applyDsqlVariants(
      payload.sql,
      payload.variants,
      { params: {}, input: {} },
    ),
  ).toThrow(
    "missing dsql variant value at input.movie_info.clause.where.id.op",
  );
});

test("rejects invalid dsql variant values", () => {
  expect(() =>
    applyDsqlVariants(payload.sql, payload.variants, {
      params: {},
      input: {
        movie_info: {
          clause: {
            where: {
              id: {
                op: "gt",
              },
            },
          },
        },
      },
    }),
  ).toThrow("invalid dsql variant value at input.movie_info.clause.where.id.op: gt");
});

test("treats variant replacement text literally", () => {
  expect(
    applyDsqlVariants(
      "select '{{params.variant}}', '{{params.variant}}'",
      {
        "params.variant": {
          cases: { literal: "$&-$$" },
        },
      },
      { params: { variant: "literal" } },
    ),
  ).toBe("select '$&-$$', '$&-$$'");
});
