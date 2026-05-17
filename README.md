# DSQL

**DSQL** is an early-stage **Domain Specific Query Language** for generating SQL-native,
typed application data access from relational schemas.

This is early work in progress. The language, generated metadata, and integration
APIs are still being shaped.

The goal is not to hide SQL or abstract every possible backend. The goal is to
make relational databases, especially Postgres-style schemas, easier to expose as
typed, policy-aware, frontend-friendly APIs.

## Why

Modern app stacks often have to choose between:

- writing SQL by hand and manually maintaining TypeScript types, endpoints, and
  cache behavior
- adopting GraphQL and a separate schema/runtime layer
- using an ORM that is type-safe but still leaves API shape, permissions, and
  frontend data fetching mostly hand-written
- generating broad CRUD APIs that become hard to refine as product behavior gets
  more specific

DSQL explores a narrower path:

> SQL-native, compile-time API/query generation from a small shape language.

The database remains the source of truth. DSQL reads schema metadata, checks
queries, generates SQL, and can emit metadata for clients, endpoints, validation,
editor tooling, and framework integrations.

DSQL itself is intended to be framework and language agnostic. Some integrations
will likely become more mature than others, but the core handoff is generated
SQL plus JSON metadata. Any toolchain or language can build on that.

`dsql generate` compiles a project, writes an inspectable build manifest under
`dsql/build/manifest.json`, and runs configured generator commands. DSQL owns
the checked SQL and metadata; host integrations own rendering framework-specific
files.

```toml
[generate.typescript]
enabled = true
out_dir = "src/generated/dsql"
cmd = ["bun", "scripts/dsql-generate.ts"]
```

## Simple Example

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

## Frontend Shape

One target integration is modern TypeScript frameworks.

```ts
const UsersPage = dsql`
  query UsersPage {
    users(limit $$limit) {
      id
      name
    }
  }
`;
```

A Vite, Babel, SWC, or similar plugin could compile inline DSQL into a typed
query helper.

```ts
const users = useQuery(UsersPage.queryOptions({ limit: 20 }));
```

This is meant to work with libraries such as TanStack Query without forcing a
GraphQL runtime or normalized client cache.

## Language Integrations

TypeScript is one natural target, but it is not the only one. A Rust integration
could expose a compile-time macro that checks the query and generates a small
typed module.

```rust
dsql::query! {
    pub mod users_page {
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
