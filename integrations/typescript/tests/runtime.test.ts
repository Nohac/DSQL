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
  }
>;

const MovieInfoOperation = {
  id: "movie-info-hash",
  name: "MovieInfo",
  kind: "query",
} satisfies MovieOperation;

const payload = {
  operation: MovieInfoOperation,
  sql: "select * from movie_info where id {{input.movie_info.clause.where.id.op}} $1",
  parameters: [{ path: "input.movie_info.clause.where.id.value" }],
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
  expect(materializeDsqlQuery(payload, variables)).toEqual({
    sql: "select * from movie_info where id = $1",
    values: [42],
  });
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
