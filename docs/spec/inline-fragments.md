# Inline Fragments

Status: consideration.

Inline fragments may be useful if dsql gains polymorphic result shapes such as
views, unions, interfaces, or search results that can return multiple object
types.

## Possible Shape

```dsql
query Search {
  search {
    ... on users {
      id
      name
    }
    ... on posts {
      id
      title
    }
  }
}
```

Open questions:

- Whether dsql needs polymorphic result shapes.
- How inline fragments interact with catalog tables and views.
- How result types are generated.
- Whether this can be deferred indefinitely unless a real provider requires it.

