# Functions

Status: consideration.

Functions are typed expression calls that can appear where values are valid.
They are not raw SQL escape hatches.

## Motivation

Common predicates need safe SQL functions without forcing users to drop down to
SQL strings.

```dsql
query RecentPosts {
  posts(where .published_at >= now()) {
    id
    title
    published_at
  }
}
```

The compiler should resolve `now()` as a known function, infer its return type,
and validate that the surrounding operator accepts that type.

## Initial Scope

Start with a small typed function set.

```text
now() -> timestamptz
today() -> date
lower(text) -> text
concat(text, ...) -> text
```

Functions may appear as predicate values first.

```dsql
query Users {
  users(where lower(.email) like $$email_pattern) {
    id
    email
  }
}
```

Function arguments are expressions. Field paths remain parsed syntax, so the
type checker, SQL generator, formatter, LSP, and source maps can still inspect
them.

## Provider Function Metadata

The function registry should eventually come from provider/catalog metadata.

Examples:

```text
date_trunc(text, timestamptz) -> timestamptz
coalesce(T, T) -> T
```

Provider metadata must describe argument types, return type, volatility if it
matters for code generation, and the SQL lowering name.

## Codegen Notes

Function metadata should let generated clients and tooling understand expression
types without parsing provider SQL.

Possible shape:

```json
{
  "functions": {
    "now": {
      "args": [],
      "returns": "timestamptz",
      "provider": "postgres"
    },
    "lower": {
      "args": ["text"],
      "returns": "text",
      "provider": "postgres"
    }
  }
}
```

For query-specific metadata, only functions that affect generated input or
result contracts need to be surfaced.

## Non-Goals

Do not support arbitrary raw SQL snippets as functions.

Avoid string interpolation inside literals:

```dsql
posts(where .title like "%${~name}%")
```

Prefer explicit expression calls:

```dsql
posts(where .title like concat("%", ~name, "%")) {
  id
}
```

## Open Questions

- Whether functions are global, provider-scoped, project-configured, or all of
  those.
- How overloaded functions are represented and selected.
- Whether function calls can be selected as computed fields or only used in
  predicates initially.
- How much PostgreSQL-specific function behavior should be exposed before other
  providers exist.
