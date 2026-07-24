import { expect, test } from "bun:test";
import defaultCases from "../../../tests/conformance/input-defaults.json" with {
  type: "json",
};
import {
  defineDsqlQuery,
  type DsqlQueryDefinition,
} from "../src/index";
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
    required: true,
    nullable: false,
  },
  {
    path: "input.movie_info.clause.where.id.value",
    data_type: "int",
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
  inputs: [
    ...movieInputs,
    {
      path: "context.tenant_id",
      data_type: "uuid",
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
    materializeDsqlQuery(payload, variables, { tenant_id: "tenant-7" }),
  ).toEqual({
    sql: "select * from movie_info where id = $1 and tenant_id = $2",
    values: [42, "tenant-7"],
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
      required: false,
      nullable: true,
      default: { kind: "null" },
    },
    {
      path: "params.limit",
      data_type: "int",
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
        required: false,
        nullable: false,
        default: { kind: "number", value: "10" },
      },
      {
        path: "params.search",
        data_type: "predicate",
        required: false,
        nullable: true,
        null_identity: "empty_object",
      },
      {
        path: "params.order",
        data_type: "order",
        collection: true,
        required: false,
        nullable: true,
        null_identity: "empty_collection",
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

test("legacy query definitions also materialize cache-key defaults", () => {
  const definition = {
    name: "LegacySearch",
    params: [
      {
        path: "params.limit",
        data_type: "int",
        enum_values: [],
        required: false,
        nullable: false,
        default: { kind: "number", value: "10" },
      },
    ],
    context: [],
  } as unknown as DsqlQueryDefinition<{ readonly limit?: number }>;
  const query = defineDsqlQuery(definition);

  expect(query.key({})).toEqual(query.key({ limit: 10 }));
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
