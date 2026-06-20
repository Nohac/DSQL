# Aggregates

Status: consideration.

Aggregates let a selection transform a related collection into computed output
values. The main use case is nested API shape, not replacing SQL reporting
queries.

## Selection Pipe Blocks

Aggregate syntax should use a pipe block on a relation selection.

```dsql
query Users {
  users(where .name like $$ limit 10) {
    id
    name

    post_stats: posts | aggregate {
      count
      latest_post: max .created_at
    } |
  }
}
```

Meaning:

- `posts` resolves as a relation from `users`.
- The pipe block transforms the related `posts` collection.
- `aggregate { ... }` produces one embedded object for that relation scope.
- The output key is the alias when provided, otherwise the relation output key.

Conceptual output:

```json
{
  "id": 1,
  "name": "Ada",
  "post_stats": {
    "count": 3,
    "latest_post": "2026-01-01T12:00:00Z"
  }
}
```

Pipe blocks are selection transforms only. They are not valid in `where`,
`order by`, `limit`, or `offset` clauses.

## Flattening

A spread-like marker can flatten a pipe output into the parent object.

```dsql
query Users {
  users(where .name like $$ limit 10) {
    id
    name

    ...posts | aggregate {
      post_count: count
      latest_post: max .created_at
    } |
  }
}
```

Conceptual output:

```json
{
  "id": 1,
  "name": "Ada",
  "post_count": 3,
  "latest_post": "2026-01-01T12:00:00Z"
}
```

Flattening must perform output-key collision checks. If a flattened aggregate
field would collide with another selected output key, the compiler should report
a diagnostic and require aliases.

The spread-like syntax is intentionally similar to fragment spread: it signals
that the produced fields are merged into the current object.

## Nested Relation Aggregates

Pipe aggregates should compose inside normal relation selections.

```dsql
query Users {
  users(limit 10) {
    id
    posts {
      id
      title

      comment_stats: comments | aggregate {
        count
      } |
    }
  }
}
```

The aggregate scope is the selected relation. In the example above, `comments`
is aggregated per `posts` row.

Aggregates should also compose beside the full relation when an API needs both
summary information and a limited detail list.

```dsql
query Users {
  users(limit 10) {
    id

    ...posts | aggregate {
      post_count: count
    } |

    posts(limit 5) {
      id
      title
    }
  }
}
```

Conceptual output:

```json
{
  "id": 1,
  "post_count": 42,
  "posts": [
    { "id": 10, "title": "..." }
  ]
}
```

## Aggregate Fields

Initial aggregate fields should be small and explicit.

```dsql
aggregate {
  count
  latest_post: max .created_at
  earliest_post: min .created_at
}
```

Candidate built-ins:

- `count`
- `count .field`
- `exists`
- `max .field`
- `min .field`
- `sum .field`
- `avg .field`

Aggregate field aliases should be supported. If the inferred output key would be
unclear or collide with another aggregate output, the compiler should require an
alias.

`exists` returns a boolean indicating whether the scoped relation contains at
least one row after relation clauses are applied.

```dsql
query Users {
  users {
    id

    ...posts | aggregate {
      has_posts: exists
      post_count: count
    } |
  }
}
```

## Grouped Aggregates

Grouped aggregates return a collection of aggregate rows instead of one
aggregate object.

```dsql
query Users {
  users {
    id

    post_statuses: posts | aggregate by status {
      status
      count
      latest_post: max .created_at
    } |
  }
}
```

Meaning:

- `posts` resolves as a relation from `users`.
- `status` is the grouping key.
- Each output row represents one `status` group for that user's posts.
- `status` is selectable because it is a grouping key.
- `count` and `max .created_at` are aggregate outputs.

Conceptual output:

```json
{
  "id": 1,
  "post_statuses": [
    {
      "status": "published",
      "count": 8,
      "latest_post": "2026-01-01T12:00:00Z"
    },
    {
      "status": "draft",
      "count": 2,
      "latest_post": "2025-12-20T09:00:00Z"
    }
  ]
}
```

Ungrouped aggregate:

```dsql
posts | aggregate {
  count
} |
```

Grouped aggregate:

```dsql
posts | aggregate by status {
  status
  count
} |
```

Flattening grouped aggregates should be invalid because grouped aggregates
produce a collection, not a single object that can be merged into the parent.

## Relation Arguments

The relation before the pipe may use normal relation clauses.

```dsql
query Users {
  users {
    id

    recent_post_stats: posts(where .created_at >= "2026-01-01") | aggregate {
      count
      latest_post: max .created_at
    } |
  }
}
```

The relation clauses apply before aggregation and are scoped to that relation.

## Codegen Notes

Aggregates should contribute normal result-shape metadata. Generated clients
should not need to infer aggregate output types from SQL text.

Possible shape:

```json
{
  "result": {
    "users": {
      "fields": {
        "post_count": { "type": "int", "nullable": false },
        "latest_post": { "type": "timestamptz", "nullable": true }
      }
    }
  },
  "aggregates": [
    {
      "path": "users.posts",
      "output": "post_count",
      "function": "count"
    }
  ],
  "grouped_aggregates": [
    {
      "path": "users.posts",
      "output": "post_statuses",
      "group_by": ["status"],
      "fields": ["status", "count", "latest_post"]
    }
  ]
}
```

Flattened aggregate outputs should appear at the parent result path, with any
collision diagnostics resolved before metadata is emitted.

## Non-Goals

Pipe blocks should not become a general query language inside DSQL.

Avoid clause-level pipe blocks for now:

```dsql
query Users {
  users(where posts | aggregate { count } | > 3) {
    id
  }
}
```

Filtering should continue to use scoped predicates and purpose-built predicate
forms. DSQL is intended as a convenient subset for nested data fetching and
common API design, not a replacement for SQL.

## Open Questions

- Exact parser shape for `...relation | aggregate { ... } |`.
- Whether `count` means `count(*)` and whether `count .field` skips nulls.
- How aggregate fields should resolve relationship paths, if at all.
- Whether aggregate output is always an object or can be unwrapped when it has
  one field.
- Whether aggregate relation clauses should allow `order by`, `limit`, and
  `offset`, or only `where`.
- How generated SQL variants interact with aggregate pipe outputs.
- Whether grouped aggregate keys must always appear in the output body.
- How `order by` works on grouped aggregate output.
- Whether grouped aggregates need a `having`-style predicate later.
- How nested grouped aggregates are planned efficiently.
- Whether grouped aggregates can be paginated.
