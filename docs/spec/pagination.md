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

## Page Metadata

Queries often need page metadata beside the selected rows, such as total count
or whether another page exists.

Possible shape:

```dsql
query UsersPage {
  users(order by created_at desc limit $$limit offset $$offset) {
    id
    name
  }

  users_page: users | page_info {
    total_count
    has_next_page
  } |
}
```

The exact syntax is unresolved. The important model is that page metadata is
derived from the same collection scope as the paginated selection without
requiring the user to duplicate filter predicates manually.

## Codegen Notes

Pagination should expose enough metadata for generated clients to build page
controls without hardcoding DSQL details.

Possible shape:

```json
{
  "pagination": {
    "users": {
      "style": "offset",
      "params": ["limit", "offset"],
      "order": ["created_at"],
      "metadata": ["total_count", "has_next_page"]
    }
  }
}
```

If cursor pagination is added, metadata should describe the cursor fields,
required ordering, and whether nested follow-up fetches are supported.

Open questions:

- Exact names: `first`/`after`, `limit`/`cursor`, or another model.
- Whether cursor pagination belongs in query clauses or relation metadata.
- How to represent per-parent cursors for batched nested pagination.
- Whether pagination requires split-fetch support.
- Whether page metadata should be a pipe block, an aggregate extension, or a
  separate generated metadata artifact.
