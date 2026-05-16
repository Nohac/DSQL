# Computed Expressions

Status: consideration.

Computed expressions would let a query select scalar values that are derived
from columns or safe functions.

## Possible Shape

```dsql
query Users {
  users {
    id
    full_name: concat(first_name, " ", last_name)
  }
}
```

Derived fields could also use an explicit marker if plain function-like syntax
becomes ambiguous with catalog-backed computed fields.

```dsql
query Titles {
  title {
    id
    decade: derive (.production_year / 10) * 10
  }
}
```

Computed expressions are intentionally deferred because they can quickly become
a full SQL expression language.

Open questions:

- Which functions are built in.
- How provider-specific functions are exposed.
- How expression types are inferred.
- How to avoid raw SQL string interpolation.
- Whether computed fields should come from catalog/provider metadata instead.
- Whether selected computed expressions use plain expression syntax or require
  an explicit marker such as `derive`.
