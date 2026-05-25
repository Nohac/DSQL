# DSQL Query Cheat Sheet

DSQL is a GraphQL-shaped query language for SQL databases. Write selections in the shape of the JSON result you want. Field names come from the database catalog: tables, columns, and foreign-key relation names.

## Primary Commands

Validate the whole project without writing generated files:

```sh
dsql validate
```

Use this first when checking DSQL changes. It scans configured project documents, including embedded DSQL in TypeScript, and prints parse/check/lint/plan diagnostics such as unknown fields or unindexed joins.

Generate artifacts manually:

```sh
dsql generate
```

If `bun dev` / Vite is already running with the DSQL plugin, do not run `dsql generate`; Vite keeps the compiler daemon hot and regenerates on file changes.

Validate a single `.dsql` file:

```sh
dsql check path/to/query.dsql
```

Format a single `.dsql` file:

```sh
dsql fmt path/to/query.dsql
```

## Basic Query

```dsql
query MovieInfoLookup {
  movie_info(where .id == 1811435 limit 1) {
    id
    info
    note
    title {
      id
      title
      production_year
    }
  }
}
```

Rules:
- Every executable operation is a named `query`.
- Root selections are table names.
- Scalar selections are column names.
- Nested selections are relation names.
- Relation names are exact catalog relation names. Do not singularize or pluralize them.

## Clauses

Clauses go in parentheses after a table or relation selection.

```dsql
query Movies {
  title(where .production_year >= 2000 order by title asc limit 10 offset 5) {
    id
    title
    production_year
  }
}
```

Supported clauses:

```dsql
where .field == value
where .field != value
where .field > value
where .field >= value
where .field < value
where .field <= value
where .field like "%text%"
where expr and expr
where expr or expr
order by field asc
order by field desc
limit 10
offset 20
```

## Paths

Use path prefixes in predicates:

```dsql
.field        # current table/relation
.rel.field    # related table through a relation path
~field        # root row, useful inside nested relations
..field       # outer/current parent scope where supported
```

Examples:

```dsql
query Companies {
  company_name(where .movie_companies.title.production_year == 2006) {
    id
    name
  }
}
```

```dsql
query Users {
  users {
    id
    posts(where .user_id == ~id) {
      id
      title
    }
  }
}
```

## Variables

Use `$` for structured input variables and `$$name` for top-level params.
Inside fragments, `$` and `$$` are fragment-local: `$` becomes fragment
`input`, and `$$` becomes fragment `params`.

```dsql
query SearchMovies {
  title(where .production_year == $ limit $) {
    id
    title
  }
}
```

Generated input shape:

```ts
{
  input: {
    title: {
      clause: {
        where: { production_year: number },
        limit: number,
      },
    },
  },
}
```

Named structured variables:

```dsql
query SearchMovies {
  title(where .production_year >= $min_year) {
    id
  }
}
```

Top-level params:

```dsql
query SearchMovies {
  title(where .title like $$search) {
    id
    title
  }
}
```

Generated params shape:

```ts
{
  params: {
    search: string;
  }
}
```

Nested relation clause variables include `body` in generated input paths:

```dsql
query CompanyMovies {
  company_name(limit $) {
    id
    movie_companies(limit $) {
      movie_id
    }
  }
}
```

Generated input shape:

```ts
{
  input: {
    company_name: {
      clause: { limit: number },
      body: {
        movie_companies: {
          clause: { limit: number },
        },
      },
    },
  },
}
```

## Operator Variables

Use an operator allowlist when the operator itself is dynamic.

```dsql
query SearchMovies {
  title(where .production_year $[==, >, >=] $year) {
    id
    title
  }
}
```

Named operator variable:

```dsql
query SearchMovies {
  title(where .title $$title_op[==, !=, like] $title) {
    id
  }
}
```

Sort direction can also be dynamic:

```dsql
query OrderedMovies {
  title(order by title $direction) {
    id
    title
  }
}
```

Allowed sort values are `asc` and `desc`.

## Aliases

Use aliases to rename output fields or disambiguate duplicate selections.

```dsql
query MovieTitles {
  title(limit 10) {
    id
    name: title
  }
}
```

Aliases also affect generated result and input paths.

## Qualified Relations

If multiple foreign keys point to the same table, use a qualified relation path.

```dsql
query MovieRelationPathSelector {
  movie_info(where .id == 1811435 limit 1) {
    id
    title {
      aliases: aka_title::movie_id {
        id
      }
      episodes: aka_title::episode_of_id {
        id
      }
    }
  }
}
```

## Fragments

Fragments define reusable selections for a table.

```dsql
fragment MovieCompanyFields on movie_companies {
  note
  company_type {
    kind
  }
}

query CompanyMovies {
  company_name(limit 5) {
    id
    name
    movie_companies(limit 3) {
      company_id
      ...MovieCompanyFields
    }
  }
}
```

Fragment rules:
- `fragment Name on table_name`.
- Spread with `...Name`.
- The fragment table must match the current selection table.
- Fragments can live in other indexed project files.
- Fragment definitions are not executable operations, but generation emits
  fragment metadata and TypeScript fragment values/types.

Generated TypeScript exposes fragment values with a `Fragment` suffix and result
types with a `FragmentResult` suffix:

```ts
import {
  dsql,
  DsqlFragment,
  MovieCompanyFieldsFragment,
  MovieCompanyFieldsFragmentResult,
} from "./generated/dsql/queries";

const MovieCompanyFields = dsql(`
fragment MovieCompanyFields on movie_companies {
  note
  company_type {
    kind
  }
}
`);

MovieCompanyFields satisfies typeof MovieCompanyFieldsFragment;

type Props = {
  movieCompany: DsqlFragment<typeof MovieCompanyFields>;
};

const item: MovieCompanyFieldsFragmentResult = {
  note: null,
  company_type: { kind: "production companies" },
};
```

Use `DsqlFragment<typeof FragmentValue>` in app code. A `const` value such as
`MovieCompanyFields` cannot be used directly as a TypeScript type name.

Fragments can also define their own variables:

```dsql
fragment DashboardMetricFields on title {
  cast_info(limit $cast_limit) {
    person {
      name
    }
  }
  movie_info(where .info like $$info_search) {
    info
  }
}

query DashboardMetricsQuery {
  movie_info_idx(limit 10) {
    title {
      ...DashboardMetricFields
    }
  }
}
```

Generated fragment variable types are standalone:

```ts
export type DashboardMetricFieldsFragmentParams = {
  info_search: string;
};

export type DashboardMetricFieldsFragmentInput = {
  cast_info: {
    clause: {
      limit: {
        cast_limit: number;
      };
    };
  };
};
```

When an operation spreads a fragment, fragment variables are passed under the
spread site in operation `input`. Fragment `$$` does not become an operation
top-level `params` field:

```ts
export type DashboardMetricsQueryInput = {
  movie_info_idx: {
    body: {
      title: {
        body: {
          DashboardMetricFields: {
            params: DashboardMetricFieldsFragmentParams;
            input: DashboardMetricFieldsFragmentInput;
          };
        };
      };
    };
  };
};
```

If a fragment only uses `$` variables and no `$$` variables, the spread-site
branch can be just the fragment input alias:

```ts
DashboardMetricFields: DashboardMetricFieldsFragmentInput;
```

Use `DsqlFragmentVariables<typeof FragmentValue>` when a caller needs the full
fragment variable envelope.

## Embedded DSQL In TypeScript

DSQL can be embedded in TypeScript with `dsql(...)` or tagged template syntax.

```ts
const BetterQuery = dsql(`
query BetterQuery {
  company_name(limit 5) {
    id
    name
  }
}
`);
```

Generated operation values are named `<QueryName>Operation`; generated fragment
values are named `<FragmentName>Fragment`.

```ts
import { dsql, DsqlOperationResult } from "./generated/dsql/queries";

const BetterQuery = dsql(`
query BetterQuery {
  company_name(limit 5) {
    id
    name
  }
}
`);

type BetterQueryResult = DsqlOperationResult<typeof BetterQuery>;
```

For precise fragment result inference, prefer the function-call form
`dsql(\`fragment ...\`)`.

Do not use JavaScript interpolation inside DSQL templates.

```ts
// Bad
dsql(`query Q { users(where .id == ${id}) { id } }`);

// Good
dsql(`query Q { users(where .id == $) { id } }`);
```

## TanStack Query And SSR

Generated TanStack helpers expose three related entry points:

```ts
import {
  executeQuery,
  queryOptions,
  useQuery,
} from "./generated/dsql/queries";
import { MovieSearch } from "./imdb-dsql";
```

Use `useQuery` in components:

```ts
const result = useQuery(MovieSearch, {
  params: { search: "%star%" },
  input: {},
});
```

Use `queryOptions` for TanStack Query cache and SSR hydration. It returns the
object shape TanStack expects, including both `queryKey` and `queryFn`.

```ts
loader: async ({ context }) => {
  await context.queryClient.ensureQueryData(
    queryOptions(MovieSearch, {
      params: { search: "%star%" },
      input: {},
    }),
  );
};
```

Use `executeQuery` only when you want direct execution without TanStack Query
cache or hydration:

```ts
const data = await executeQuery(MovieSearch, {
  params: { search: "%star%" },
  input: {},
});
```

Do not pass `executeQuery(...)` to `ensureQueryData(...)`; it returns a
`Promise<Result>`, while `ensureQueryData(...)` needs query options.

## Common Patterns

List rows:

```dsql
query RecentMovies {
  title(order by production_year desc limit 20) {
    id
    title
    production_year
  }
}
```

Filter by related row:

```dsql
query MoviesByCompany {
  title(where .movie_companies.company_name.name == $$company) {
    id
    title
  }
}
```

Nested limited relation:

```dsql
query CompaniesWithMovies {
  company_name(limit 10) {
    id
    name
    movie_companies(limit 5) {
      movie_id
      note
    }
  }
}
```

## Quick Do / Don't

Do:
- Use exact table, column, and relation names from the catalog.
- Name every query.
- Name fragments that should get generated TypeScript values/types.
- Use `$` / `$name` for structured input.
- Use `$$name` for top-level operation params, or fragment-local params inside
  fragments.
- Use fragments for repeated selection sets.
- Type fragment props as `DsqlFragment<typeof SomeFragment>`.
- Reuse generated fragment input/params types for spread-site variables.
- Use `queryOptions(operation, vars)` with `ensureQueryData` for SSR hydration.
- Use `executeQuery(operation, vars)` for direct, uncached server execution.
- Use aliases for duplicate output names.

Don't:
- Invent singularized relation names.
- Put subselections on scalar columns.
- Select unknown columns.
- Use JS interpolation inside embedded DSQL.
- Assume nested input paths skip `body`.
- Assume fragment `$$` variables become operation top-level params.
- Use `$[*]` as an operator allowlist.
- Use a fragment `const` directly as a TypeScript type name.
- Pass `executeQuery(...)` directly to `ensureQueryData(...)`.
