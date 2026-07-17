# Aggregates

Status: proposed.

Aggregates transform a collection-producing selection into computed output.
The source may be a root table or a nested relation. The main use case is
typed API output shape, not unrestricted reporting queries.

## Selection Transform

An aggregate is a pipe transform on a collection source:

```dsql
query Users {
  users(limit 10) {
    id
    name

    post_stats: posts | aggregate {
      count
      latest_post: max .created_at
    }
  }
}
```

`posts` resolves normally from `users`. Its rows are filtered by any permitted
source clauses and then transformed into one aggregate object:

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

The output key is the explicit alias when present and otherwise the source
selection's ordinary output key.

Aggregation requires a collection-producing source. Piping a catalog-proven
singular relation is a cardinality diagnostic. This is the inverse of ordinary
relation flattening, which requires a singular object result.

Only one transform may follow a source selection. General pipe chaining is not
part of this feature.

## Root Aggregates

Root table collections may be aggregated in the same way as related
collections:

```dsql
query UserStats {
  stats: users(where .active == true) | aggregate {
    count
    latest_signup: max .created_at
  }
}
```

Conceptual output:

```json
{
  "stats": {
    "count": 42,
    "latest_signup": "2026-07-17T08:00:00Z"
  }
}
```

A normal root selection is collection-valued. An ungrouped root aggregate
changes that root field to one non-null object. SQL generation may still render
root selections as independent statements; the contract is the assembled
result object, not a promise that every root shares one SQL statement.

Grouped root aggregates are keyed arrays:

```dsql
query UsersByStatus {
  users_by_status: users | aggregate by .status {
    count
  }
}
```

An empty source produces `[]`, as it does for a grouped nested aggregate.

## Flattened Output

The query language's general `...` form merges an object-valued result into its
parent. Since an ungrouped aggregate produces one object, it may be flattened:

```dsql
query Users {
  users(limit 10) {
    id

    ...posts | aggregate {
      post_count: count
      latest_post: max .created_at
    }
  }
}
```

```json
{
  "id": 1,
  "post_count": 42,
  "latest_post": "2026-01-01T12:00:00Z"
}
```

Root aggregate fields may likewise merge into the query result object:

```dsql
query UserStats {
  ...users | aggregate {
    user_count: count
    latest_signup: max .created_at
  }
}
```

Flattened aggregate fields participate in the same output-key length and
collision checks as direct selections and fragment-contributed selections.
Aggregate collisions are diagnostics and never merge implicitly.

The full flattening contract, including singular-relation nullability, is in
[Query Language](query.md#flattened-relation-selections).

## Aggregate Fields

The first aggregate functions are deliberately small:

```dsql
aggregate {
  count
  populated: count .published_at
  has_posts: exists
  latest_post: max .created_at
  earliest_post: min .created_at
}
```

Initial function semantics:

| Form | Meaning | Empty input | Logical type | Nullable |
|---|---|---|---|---|
| `count` | Count source rows, like `count(*)` | `0` | `int` | no |
| `count .field` | Count non-null field values | `0` | `int` | no |
| `exists` | Whether at least one source row exists | `false` | `boolean` | no |
| `min .field` | Minimum field value | `null` | field type | yes |
| `max .field` | Maximum field value | `null` | field type | yes |

PostgreSQL returns `bigint` for both count forms. They use dsql's current
logical `int` contract, including its existing host-number precision limits.

`count` and `exists` infer the output keys `count` and `exists`. Operand forms
require an explicit alias in the first implementation; the compiler does not
invent names such as `max_created_at`.

Aggregate output keys are ordinary PostgreSQL result keys. They must be at most
63 bytes and must not collide with another result field at their final output
path.

### Operands And Types

Initial operands are `.`-anchored direct scalar columns on the aggregate
source. Relationship traversal, parent/root anchors, and object-valued operands
are diagnostics. Each function accepts only operand types supported by the
selected database provider.

`count .field` accepts any scalar column. The initial `min` and `max` allowlist
is `int`, `text`, and `timestamptz`. `boolean`, `json`, `uuid`, and `unknown`
are diagnostics until provider capability metadata explicitly supports them.

`sum` and `avg` are planned additions, not part of the first function set.
They require logical `Numeric` and `Float` types and PostgreSQL return-type
rules. That foundation also fixes the existing `unknown` typing of ordinary
numeric columns and should land independently of aggregate planning.

### Numeric And Float Wire Types

Exact PostgreSQL `numeric`/`decimal` values use dsql's logical `numeric` type.
They are serialized as JSON strings and generate TypeScript `string`. Finite
values use their decimal text; PostgreSQL's supported non-finite values use the
exact tags `NaN`, `Infinity`, and `-Infinity`. The SQL renderer casts exact
numerics to text at the JSON construction boundary so the database never rounds
them through a JSON or JavaScript number. Numeric inputs use the same string
contract; PostgreSQL infers or receives the target numeric type from the
expression in which the parameter is used.

PostgreSQL `real`/`float4` and `double precision`/`float8` use the distinct
logical `float` type. Finite values remain JSON numbers. PostgreSQL serializes
non-finite values as the JSON strings `NaN`, `Infinity`, and `-Infinity`, so the
generated TypeScript type is
`number | "NaN" | "Infinity" | "-Infinity"`.

Float inputs accept that same union. The string tags are the reliable transport
form for non-finite inputs because JSON serialization turns JavaScript
`NaN`/infinities into `null`; PostgreSQL accepts the tags as parameter text.
Keeping finite floats as numbers preserves normal host ergonomics while the
distinct logical type avoids weakening exact numerics to floating point.

Function vocabulary is contextual inside aggregate positions. `aggregate`,
`count`, `exists`, `min`, `max`, `sum`, and `avg` must not become globally
reserved lexer tokens, because ordinary catalog columns may use those names.
`by` remains the existing grammar keyword.

Empty `aggregate {}` bodies are diagnostics.

## Source Clauses

The initial aggregate source permits only `where`:

```dsql
query Users {
  users {
    id

    recent_post_stats: posts(where .created_at >= "2026-01-01") | aggregate {
      count
      latest_post: max .created_at
    }
  }
}
```

The filter is scoped to the source collection and applies before aggregation.
`order by`, `limit`, and `offset` are diagnostics in this first contract:
ordering alone does not affect an aggregate, while slicing would define a
different source-subquery feature.

Aggregate inputs are never capped by a renderer's collection safety limit.
Such a limit may protect returned arrays, but applying it before aggregation
would silently corrupt counts and other aggregate values.

## Nested Composition

Aggregates compose at every normal relation depth:

```dsql
query Users {
  users(limit 10) {
    id
    posts {
      id
      title

      comment_stats: comments | aggregate {
        count
      }
    }
  }
}
```

Here `comments` is aggregated independently for each `posts` row.

Summary and detail selections may coexist:

```dsql
query Users {
  users(limit 10) {
    id

    ...posts | aggregate {
      post_count: count
    }

    posts(limit 5) {
      id
      title
    }
  }
}
```

The aggregate and detail selections are independent scopes. The detail limit
does not affect `post_count`.

## Grouped Aggregates

Grouped aggregates transform the source collection into an array of aggregate
rows. Group keys are declared after `by` and are automatically emitted into
each row:

```dsql
query Users {
  users {
    id

    post_statuses: posts | aggregate by .status {
      count
      latest_post: max .created_at
    }
  }
}
```

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

Multiple direct scalar keys and aliases use this conceptual shape:

```dsql
posts | aggregate by state: .status, .category {
  count
}
```

The body contains aggregate functions only. Repeating a group key in the body
is invalid. Group-key aliases define their output keys; unaliased keys use the
terminal column name. Group-key and aggregate-field collisions are diagnostics.
Relationship-traversing group keys such as `.author.name` are not supported
initially. `exists` is invalid in a grouped body because every emitted group is
non-empty and the value would always be `true`.

Every group contains at least one source row. `count` is therefore non-null.
`min` and `max` over a not-null operand are non-null in grouped output; they are
nullable when the operand column is nullable. A nullable group key forms a SQL
`NULL` group, is emitted as `null`, and retains the column's nullability.

An empty source produces `[]`. Grouped output is array-valued and therefore
cannot use `...`; that follows from the general rule that only object-valued
results can flatten.

Initial grouped aggregates have no `having`, group-result pagination, or group
ordering. As with an ordinary relation lacking `order by`, row order is
unspecified. These additions can build on the grouped result model without
changing its basic shape.

## Fragments And Directives

Fragments may contain flattened relation selections and aggregate pipe
selections. Their produced keys participate in collision checking when the
fragment is expanded at a spread site.

Aggregate bodies contain aggregate fields only; fragment spreads are invalid
inside them.

The first aggregate implementation rejects directives on pipe transforms and
inside aggregate bodies. Aggregate directive locations and conditional output
shape require a separate specification before they can be enabled.

## Variables And Selection Identity

Aggregation and flattening change result shape, not source resolution. Clause
variables remain attached to the semantic source selection.

Keyed aggregate selections retain the ordinary alias/output-key selection path.
Flattened aggregates have no output wrapper, so their variables use the
underlying root or relation scope plus an aggregate scope segment. Repeated
flattened scopes, or a flattened scope beside a keyed selection using the same
path, must diagnose inferred-input ambiguity rather than silently making one
input control two different filters.

The complete inference rule is in
[Variables](variables.md#flattened-and-transformed-selections).

## Parser Classification

Ellipsis selections are classified by the syntax following their relation or
fragment name:

```text
...Name                         fragment spread
...relation { ... }             flattened relation selection
...relation | aggregate { ... } flattened aggregate selection
```

Schema qualification and relation-edge selectors are valid on flattened
relations. A fragment name remains one unqualified name.

Future fragment binding lists and relation clause lists share a parenthesized
prefix but have disjoint first tokens: bindings begin with `$` or `$$`, while
selection clauses begin with clause keywords. Empty ambiguous parentheses are a
diagnostic. An ellipsis selection classified as a relation but followed by
neither a selection set nor a pipe is also a diagnostic. `aggregate` followed
by `by` classifies the grouped form.

An alias combined with a flattened selection set or pipe is invalid because
flattening has no wrapper key. This does not prevent the separately proposed
`alias: ...Fragment` form for wrapped fragment spreads.

## Result Metadata

Aggregate outputs are ordinary result-shape fields. Generated clients read the
same path, kind, logical type, and nullability metadata used for catalog fields:

- keyed ungrouped aggregate: non-null object plus scalar child fields;
- flattened ungrouped aggregate: scalar fields at the parent result path;
- grouped aggregate: non-null array of object rows with group-key and aggregate
  fields.

Public `aggregates` or `grouped_aggregates` provenance sidecars are not part of
the metadata contract. They would duplicate result shape and become ambiguous
when source paths differ from flattened output paths. Tooling-specific
provenance can be added later if a concrete consumer requires it.

## SQL Semantics

Each aggregate source is evaluated independently:

- a root aggregate is an uncorrelated aggregate query;
- a nested aggregate is correlated to its parent relation scope;
- an ungrouped source produces one aggregate row even for empty input;
- a grouped source produces zero or more group rows, collected into an array;
- summary and detail selections of the same relation do not share joins in a
  way that could multiply parent rows or change either source's clauses.

The semantic plan should represent aggregate outputs directly. SQL generation,
metadata assembly, services, and generated types consume that checked plan and
must not independently re-resolve aggregate names or operands.

## Scalar Aggregate Predicates

Purpose-built scalar relation aggregates are a planned later extension:

```dsql
query PopularUsers {
  users(where (.posts | count) >= $$minimum_posts) {
    id
    name
  }
}

query UsersWithPosts {
  users(where .posts | exists) {
    id
  }
}
```

The aggregate transform binds before comparison and shares the output
aggregate function, type, empty-input, and null semantics. `min` or `max` over
an empty scope yields SQL `NULL`; a comparison with that value is unknown and
excludes the parent row.

This form is reserved in the specification but is not part of the first
aggregate implementation. Clause-bearing relation paths inside predicates,
multi-step paths, and their nested variable scopes are separate increments.

## Non-Goals

Aggregate blocks are selection transforms. Object-producing pipe blocks and
general relational pipelines are not valid inside `where`, `order by`,
`limit`, or `offset`. The reserved scalar predicate forms above are a closed
allowlist, not a general clause pipeline facility.

Aggregates do not initially provide:

- relationship-path operands or group keys;
- source ordering or slicing;
- single-field object unwrapping;
- `having`, grouped pagination, or grouped ordering;
- provider-defined aggregate functions;
- general pipe composition.

## Incremental Delivery

The contract is designed for independently shippable slices:

1. logical numeric/float types and their JSON/host wire representation;
2. keyed ungrouped root and relation aggregates with
   `count`/`exists`/`min`/`max`;
3. general singular-relation flattening and flattened ungrouped aggregates;
4. grouped root and relation aggregates;
5. `sum` and `avg` after numeric return types exist;
6. scalar aggregate predicates.

The order may vary where slices are independent, but later slices must preserve
the function and result-shape semantics established by the earlier ones.

## Open Questions

- Which aggregate directive locations and merge rules are useful once
  conditional output semantics exist?
- What syntax should add grouped ordering, pagination, and `having`?
- When should aggregate operands and group keys support relationship paths?
- Which provider capability metadata should replace the initial hard-coded
  function/type allowlists?
