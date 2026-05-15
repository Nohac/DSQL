# Aggregates

Status: unfinished.

Aggregates let a query select computed values over a table or relation
collection.

## Relation Aggregate Field

```dsql
query Users {
  users {
    id
    post_count: posts.count()
  }
}
```

Meaning:

- `posts` resolves as a relation from `users`.
- `count()` applies to that relation collection.
- The output is scalar.
- An alias is expected when the generated field name would be unclear or when
  selecting multiple aggregates from the same relation.

## Aggregate Filters

Aggregates may need their own filter scope.

```dsql
query Users {
  users {
    id
    published_posts: posts.count(where .status == "published")
    draft_posts: posts.count(where .status == "draft")
  }
}
```

The aggregate filter applies inside the `posts` scope, not to the parent
`users` selection.

## Aggregate Blocks

For table-level aggregate output, a block form may be clearer.

```dsql
query PostStats {
  posts.aggregate {
    count
    max {
      created_at
    }
    min {
      created_at
    }
  }
}
```

Open questions:

- Exact function-call syntax.
- Whether aggregate blocks should use `.aggregate` or a clause form.
- How aggregate aliases are inferred.
- Which aggregate functions are built in.
- How provider-specific aggregates are exposed.
