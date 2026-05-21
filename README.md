# DSQL

**DSQL** is an early-stage **Domain Specific Query Language** for generating SQL-native,
typed application data access from relational schemas.

DSQL gives you GraphQL-like data shapes without a GraphQL server: queries are
checked at build time, compiled to SQL, and exposed as typed app code.

## Example

Write a DSQL query:

```ts
const UsersPage = dsql(`
  query UsersPage {
    users(where .active == true order by created_at desc limit $$limit) {
      id
      name
      email
    }
  }
`);
```

Use it like a typed frontend query:

```ts
const users = useQuery(UsersPage, {
  params: { limit: 20 },
  input: {},
});
```

DSQL checks the query against your database schema, turns it into real SQL, and
generates the TypeScript types and query wiring your app uses.

## Why

Most application data fetching repeats the same work in several places: write a
query, shape an endpoint, maintain TypeScript types, and wire it into frontend
fetching.

GraphQL solves part of this with client-shaped data, but it also adds a schema,
server runtime, resolver layer, and protocol. Raw SQL keeps the database close,
but the app-facing types and query wiring are still mostly manual.

DSQL takes a narrower path. The database schema stays the source of truth. You
write the data shape your app needs, and DSQL checks it, compiles it to SQL, and
generates the metadata and typed code integrations can use.

## Query Shape

```dsql
query Users {
  users(where .active == true order by created_at desc limit 20) {
    id
    name
    email
  }
}
```

Conceptually, this can generate SQL plus a typed result shape:

```ts
type UsersResult = {
  users: Array<{
    id: string;
    name: string;
    email: string | null;
  }>;
};
```

## Example Direction

```dsql
query UsersPage {
  users(
    where .tenant_id == $:tenant_id
      and filter $$search on selected
    order by $$order on selected_indexed
    limit $$limit
  ) {
    id
    name
    email

    posts(order by created_at desc limit 5) {
      id
      title
      created_at
    }
  }
}
```

The intended generated contract includes:

- SQL for the checked query
- result types
- public params such as `limit`, `search`, and `order`
- required host context such as `tenant_id`
- policy-driven nullability
- metadata for cache keys, hovers, diagnostics, and code generation

## Language Integrations

TypeScript is one natural target, but it is not the only one. A Rust integration
could expose a compile-time macro that checks the query and generates a small
typed module.

```rust
dsql::query! {
    query users_page {
        users(limit $$limit) {
            id
            name
        }
    }
}

let result = users_page::fetch(
    &pool,
    users_page::Params { limit: 20 },
).await?;

for user in result.users {
    println!("{} {}", user.id, user.name);
}
```

The exact integration API is open. It might be a procedural macro, generated
module, build script, or file-based include. The important part is that DSQL can
expose the same checked SQL and metadata to multiple host languages.

## Split Fetch

DSQL also explores independently fetchable nested branches. A master query can
hydrate initial child data during SSR, while generated child queries can later
paginate or refresh one nested relation without refetching the entire parent
query.

This is useful for shapes like users with posts, projects with tasks, or any UI
where nested collections need their own cache and pagination behavior.

## Best Fit

DSQL is likely strongest for:

- Postgres/MySQL-heavy applications
- internal tools and admin dashboards
- B2B SaaS CRUD-plus workflows
- nested read APIs over relational data
- AI-assisted app generation against an existing schema
- teams that want typed frontend data access without adopting GraphQL
- projects where readable generated SQL and migration escape hatches matter

## Status

This project is pre-1.0 and still shaping the language and architecture. The
tracked specs in [`spec/`](spec/) describe current direction and open questions.

Backwards compatibility is not a goal before 1.0.
