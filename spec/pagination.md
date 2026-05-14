# Pagination

Status: unfinished.

Pagination is the language surface for slicing ordered collections beyond basic
`limit` and `offset`.

## Cursor Pagination

```dsql
query Posts {
  posts(first 20 after $cursor order by created_at desc) {
    id
    title
  }
}
```

The exact syntax is unresolved. The important model is:

- Pagination applies to a collection.
- Pagination depends on a stable ordering.
- Pagination arguments affect only the collection they are attached to.

## Nested Pagination

Nested pagination should support fetching more items for one parent without
refetching the parent object.

```dsql
query Users {
  users(limit 20) {
    id
    name
    posts(first 5) {
      id
      title
    }
  }
}
```

A follow-up fetch should be able to load more `posts` for one or more users
without refetching `users.name`.

Open questions:

- Exact names: `first`/`after`, `limit`/`cursor`, or another model.
- Whether cursor pagination belongs in query clauses or relation metadata.
- How to represent per-parent cursors for batched nested pagination.
- Whether pagination requires split-fetch support.

