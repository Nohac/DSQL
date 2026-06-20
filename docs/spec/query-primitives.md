# Query Primitives

Status: consideration.

A query primitive is a reusable query-shaped unit that behaves like a field or
relation-aware helper.

## Possible Shape

```dsql
primitive recentPosts($limit: int = 10) on users -> posts {
  posts(limit $limit order by created_at desc) {
    id
    title
    created_at
  }
}
```

Used as a field:

```dsql
query Users {
  users {
    id
    name
    recentPosts(limit 5) {
      id
      title
    }
  }
}
```

The semantic goal is reusable relation-aware query logic, not textual macros.

Open questions:

- Whether primitives are needed if fragments and split fetches are expressive
  enough.
- Whether primitives are declarations, provider metadata, or project config.
- How primitive inputs are declared and type checked.
- How primitives compose with relation naming and aliases.

