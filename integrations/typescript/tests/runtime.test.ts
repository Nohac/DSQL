import { expect, test } from "bun:test";
import {
  applyDsqlVariants,
  collectDsqlParameterValues,
  dsqlQueryKey,
  getDsqlPath,
  materializeDsqlQuery,
  type DsqlExecutionPayload,
  type DsqlOperation,
} from "../src/runtime";

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

const OptionalOperation = {
  id: "optional-hash",
  name: "Optional",
  kind: "query",
  requiresContext: false,
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
  inputs: [
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
  ],
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
