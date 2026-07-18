import { expect, test } from "bun:test";
import {
  applyDsqlVariants,
  collectDsqlParameterValues,
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
      eq: "=",
      ne: "!=",
    },
  },
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
