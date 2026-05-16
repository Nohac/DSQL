# Aggregates

Status: consideration.

Aggregates let a selection transform a related collection into computed output
values. The main use case is nested API shape, not replacing SQL reporting
queries.

## Selection Pipe Blocks

Aggregate syntax should use a pipe block on a relation selection.

```dsql
query MovieInfo {
  movie_info(where .info like $$ limit 10) {
    id
    info

    title_stats: title | aggregate {
      count
      latest_year: max .production_year
    } |
  }
}
```

Meaning:

- `title` resolves as a relation from `movie_info`.
- The pipe block transforms the related `title` collection.
- `aggregate { ... }` produces one embedded object for that relation scope.
- The output key is the alias when provided, otherwise the relation output key.

Conceptual output:

```json
{
  "id": 1,
  "info": "...",
  "title_stats": {
    "count": 3,
    "latest_year": 2004
  }
}
```

Pipe blocks are selection transforms only. They are not valid in `where`,
`order by`, `limit`, or `offset` clauses.

## Flattening

A spread-like marker can flatten a pipe output into the parent object.

```dsql
query MovieInfo {
  movie_info(where .info like $$ limit 10) {
    id
    info

    ...title | aggregate {
      title_count: count
      latest_year: max .production_year
    } |
  }
}
```

Conceptual output:

```json
{
  "id": 1,
  "info": "...",
  "title_count": 3,
  "latest_year": 2004
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
query MovieInfo {
  movie_info(limit 10) {
    id
    title {
      id
      title

      cast_stats: cast_info | aggregate {
        count
      } |
    }
  }
}
```

The aggregate scope is the selected relation. In the example above, `cast_info`
is aggregated per `title` row.

Aggregates should also compose beside the full relation when an API needs both
summary information and a limited detail list.

```dsql
query MovieInfo {
  movie_info(limit 10) {
    id

    ...cast_info | aggregate {
      cast_count: count
    } |

    cast_info(limit 5) {
      id
      note
    }
  }
}
```

Conceptual output:

```json
{
  "id": 1,
  "cast_count": 42,
  "cast_info": [
    { "id": 10, "note": "..." }
  ]
}
```

## Aggregate Fields

Initial aggregate fields should be small and explicit.

```dsql
aggregate {
  count
  latest_year: max .production_year
  earliest_year: min .production_year
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

## Relation Arguments

The relation before the pipe may use normal relation clauses.

```dsql
query MovieInfo {
  movie_info {
    id

    recent_title_stats: title(where .production_year > 2000) | aggregate {
      count
      latest_year: max .production_year
    } |
  }
}
```

The relation clauses apply before aggregation and are scoped to that relation.

## Non-Goals

Pipe blocks should not become a general query language inside DSQL.

Avoid clause-level pipe blocks for now:

```dsql
query Movies {
  movie_info(where title | aggregate { count } | > 3) {
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
- Whether grouping belongs in aggregate pipe blocks or remains a separate future
  feature.
