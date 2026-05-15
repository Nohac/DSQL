# Variables

Status: unfinished.

Variables allow query input values to be named, typed, and bound at execution or
generation time.

## Intended Shape

Queries may eventually declare variables.

```dsql
query UserById($id: uuid) {
  users(where .id == $id) {
    id
    name
  }
}
```

Variables may also have defaults.

```dsql
query Users($limit: int = 20) {
  users(limit $limit) {
    id
    name
  }
}
```

## Values To Consider

```dsql
$id
[1, 2, 3]
{ id: 1 }
```

Open questions:

- Exact variable declaration grammar.
- Whether variable defaults allow only literals or richer expressions.
- How variable nullability should be represented.
- Whether compound values belong in the query language or only in filter/input
  positions.
- How provider-specific scalar types are named.

User values must be emitted as SQL parameters when this is implemented.
