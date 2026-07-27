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
  collectDsqlParameterValues,
  dsqlQueryKey,
  getDsqlPath,
  materializeDsqlBindings,
  materializeDsqlQuery,
  type DsqlExecutionPayload,
  type DsqlOperation,
} from "../src/runtime";

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
  inputs: [
    ...movieInputs,
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
  inputs: optionalInputs,
} satisfies DsqlExecutionPayload<OptionalOperation>;

test("materializes defaults and nullable sql variants without mutating inputs", () => {
  const variables = {};
  expect(materializeDsqlQuery(optionalPayload, variables, {})).toEqual({
    sql: "select * from movie_info order by case when 'null' = 'asc' then id end asc limit $1",
    values: [10],
  });
  expect(variables).toEqual({});

  expect(
    materializeDsqlQuery(optionalPayload, { params: { limit: null } }, {}),
  ).toEqual({
    sql: "select * from movie_info order by case when 'null' = 'asc' then id end asc limit $1",
    values: [null],
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
  } satisfies DsqlOperation;
  const dynamicPayload = {
    operation,
    sql: "select $1 where {{dynamic:0}} and {{dynamic:1}} order by {{dynamic:2}}",
    parameters: [{ path: "params.tenant" }],
    variants: {},
    dynamicInputs,
    inputs: operation.inputs,
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
  });

  const withIdDataType = (dataType: string) => ({
    ...dynamicPayload,
    dynamicInputs: dynamicPayload.dynamicInputs.map((input) =>
      input.path === "params.search"
        ? {
            ...input,
            fields: input.fields.map((field) =>
              field.key === "id"
                ? {
                    ...field,
                    data_type: dataType,
                    wire: {
                      encoding: dataType === "int" ? "integer" : dataType,
                    },
                  }
                : field,
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

  const textCastPayload = {
    ...dynamicPayload,
    dynamicInputs: dynamicPayload.dynamicInputs.map((input) =>
      input.path === "params.search"
        ? {
            ...input,
            fields: input.fields.map((field) =>
              field.key === "id"
                ? {
                    ...field,
                    data_type: "text",
                    wire: {
                      encoding: "text_cast",
                      provider_type: {
                        schema: "pg_catalog",
                        name: "date",
                      },
                    },
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
      materializeDsqlBindings([testCase.field], {});
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
      materializeDsqlBindings([testCase.field], testCase.bindings);
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
  };
  const payload: DsqlExecutionPayload<typeof operation> = {
    operation,
    sql: dynamicCases.operation.sql.text,
    parameters: dynamicCases.operation.sql.parameters,
    variants: {},
    dynamicInputs: dynamicCases.operation.dynamic_inputs,
    inputs: dynamicCases.operation.params,
  };

  for (const testCase of dynamicCases.cases) {
    const materialize = () =>
      materializeDsqlQuery(payload, testCase.bindings, {});
    if ("expected_sql" in testCase) {
      expect(materialize(), testCase.name).toEqual({
        sql: testCase.expected_sql,
        values: testCase.expected_values,
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

  const materialized = materializeDsqlBindings(optionalInputs, bindings) as {
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
  expect(materializeDsqlBindings([], bindings)).toBe(bindings);
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
    expect(() => materializeDsqlBindings([defaulted], { params })).toThrow(
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
    ),
  ).toEqual({ params: { info: null } });
  expect(() =>
    materializeDsqlBindings([requiredNullable], { params: {} }),
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
