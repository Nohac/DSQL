# Scoped Predicates

Status: unfinished.

Scoped predicates extend `where` clauses so predicates can reference the current
selection table, related tables, parent selection tables, and the root selection
table.

This is clause syntax only. Selection sets continue to use normal field and
relation selections without scope prefixes.

## Motivation

A query should be able to filter a root collection by related rows.

```dsql
query Users {
  public::users(where .posts.title like "%foo%") {
    id
    posts {
      id
    }
  }
}
```

The `.posts.title` path means users that have at least one related `posts` row
whose `title` matches the predicate.

Nested relation clauses should also be able to reference the root selection.

```dsql
query Users {
  public::users(where .posts.title like "%foo%") {
    id
    admin

    posts(where ~admin == true) {
      id
    }
  }
}
```

The `~admin` path resolves against the root `public::users` selection, not the
nested `posts` selection.

## Scope Prefixes

Predicate field paths may start with a scope prefix.

```text
.field        current clause table
.rel.field    relationship traversal from the current clause table
..field       immediate parent selection table
~field        root selection table
~rel.field    relationship traversal from the root selection table
```

`..field` is supported as a relative parent escape hatch, but examples should
prefer `~field` when the intent is to reference the root selection. Root
references are usually clearer in deeply nested queries.

Bare field names are not valid predicate paths. Predicate field references must
start with `.`, `..`, or `~`. This keeps predicate path parsing explicit and
lets completion use the scope prefix as the signal that the user is starting a
field path.

## Relationship Predicates

Relationship paths in predicates traverse catalog relationship metadata.

```dsql
query Users {
  users(where .posts.title like "%foo%") {
    id
  }
}
```

For to-many relationships, this is an existential predicate:

```sql
where exists (
  select 1
  from posts
  where posts.user_id = users.id
    and posts.title like '%foo%'
)
```

For to-one relationships, the compiler may emit an equivalent join or exists
predicate as long as result semantics are preserved.

Relationship paths may use the same relationship references as selection sets,
including schema-qualified table references and relation edge selectors.

```dsql
query Users {
  users(where .posts.comments.body like "%question%") {
    id
  }
}
```

Defined relationship aliases, if added, should also be valid in predicate paths.

Scoped predicates may filter by related data while returning a different
relation shape.

```dsql
query Users {
  users(where .posts.comments.body like $$search limit 10) {
    id
    name

    posts(order by created_at desc limit 3) {
      id
      title
      created_at
    }
  }
}
```

SQL-style `exists` makes the related source and its clause scope explicit:

```dsql
query Users {
  users(where exists .posts(where .published == true)) {
    id
  }
}
```

An unrelated qualified table may be correlated back to the enclosing row with
the same parent and root prefixes:

```dsql
projects(
  where exists public::administrators(
    where .tenant_id == ..tenant_id
      and .user_id == $:user_id
  )
) {
  id
}
```

The source introduced by `exists` becomes the current `.` scope inside its
clauses. `..` refers to the row whose predicate contains the existence test.

The predicate selects `users` rows based on related comment text, but the
selected body still controls the returned shape independently.

## Operators

Scoped predicates use the complete core predicate operator set, including
membership, null tests, unary negation, and `like` for text pattern matching.
The grammar and empty-collection behavior are defined in [Query
Clauses](query.md#where).

The operator set stays type-aware. For example, `like` is valid for text fields
and invalid for integer fields, while an `in` collection must have an element
type compatible with its resolved field path.

## Field-To-Field Predicates

Predicates may eventually compare two resolved field paths, not only a field
path and a literal value.

```dsql
query Users {
  users(where .posts.title like "%foo%") {
    id
    admin

    posts(where .is_admin == ~admin) {
      id
    }
  }
}
```

In the nested `posts` clause, `.is_admin` resolves against the current `posts`
scope and `~admin` resolves against the root `users` scope. The generated SQL
must compare columns, not treat the right-hand side as a parameter.

Other examples:

```dsql
query Orders {
  orders(where .total > .paid_amount) {
    id
  }
}
```

Type checking should validate that both sides are comparable.

For the first implementation pass, relationship traversal should be conservative
when used on the right-hand side of a predicate. Current, parent, and root scalar
field references are enough to support correlated nested filters such as
`.is_admin == ~admin`.

## Boolean Composition

Scoped predicates use the core `where` boolean expression rules. Scoped field
paths may appear anywhere a normal predicate field path may appear.

```dsql
query Users {
  users(where .posts.title like "%foo%" and .active == true) {
    id
  }
}
```

`and` binds tighter than `or`. Parentheses may group scoped predicate
expressions explicitly.

```dsql
query Users {
  users(where (.posts.title like "%foo%" or .posts.title like "%bar%") and .active == true) {
    id
  }
}
```

Parentheses are part of the scoped predicate syntax, not only a formatter
concern. They must be preserved through parsing, planning, SQL generation, and
formatting so users can control boolean precedence.

Relationship predicates keep their normal existential meaning when composed.
For example, `.posts.title like "%foo%" and .posts.published == true` means both
conditions must hold for the filtered root row. If those paths target the same
relationship chain, the compiler should prefer SQL that applies both conditions
inside the same relationship predicate when that preserves the user-visible
semantics.

Query-authored relationship paths observe the filtered logical relation. Row
filters constrain traversed rows, and a conditionally hidden relation behaves
as empty. Filter-rule predicates deliberately resolve against raw catalog rows
instead; that evaluation boundary is defined in [Filters And Access
Rules](policies.md#rule-evaluation-boundary).

## Computed Predicate Values

String interpolation inside literals is not part of the current direction.

Avoid this form:

```dsql
posts(where .title like "%${~name}%")
```

If computed pattern values are added, prefer an explicit expression form such as
`concat`.

```dsql
posts(where .title like concat("%", ~name, "%")) {
  id
}
```

This keeps field paths visible to the parser, type checker, SQL generator, and
LSP without needing to parse expressions embedded inside string literals.

## LSP Behavior

Completion should use scope prefixes to choose the completion source.

```text
where .      current fields and relationships
where ..     parent fields and relationships
where ~      root fields and relationships
where .posts. fields and relationships on posts
```

Hover and diagnostics should resolve the full scoped path and report the table
or column that each path segment targets.

Invalid path examples:

- `.missing_field`
- `.posts.missing_field`
- `..field` when there is no parent selection table
- `~field` when there is no root table context
- `.posts.title like 10` when `title` is text

## Codegen Notes

Scoped predicates should preserve resolved path metadata for generated clients,
debug tools, and future explain/source-map features.

Possible shape:

```json
{
  "predicates": [
    {
      "path": ".posts.comments.body",
      "resolved": [
        "public::users",
        "public::posts",
        "public.comments",
        "public.comments.body"
      ],
      "operator": "like",
      "source": "query"
    }
  ]
}
```

The metadata should be stable enough for hover, diagnostics, generated filter
types, and performance tooling to point back to the original DSQL path.

## Open Questions

- Whether multi-level parent references beyond `..field` are needed.
- Whether relationship predicates need explicit quantifiers such as `some`,
  `none`, or `every`.
- Whether computed predicate values should support `concat`, SQL-style string
  concatenation, or another expression form.
- How source maps should represent predicate path segments for generated SQL,
  diagnostics, explain output, and code generation.
