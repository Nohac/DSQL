# Pipeline Queries

Status: consideration.

Some grouping and reporting workflows may fit a relational pipeline syntax
better than nested selections.

This is distinct from selection pipe blocks used for output shaping, such as
relation aggregates. Selection pipe blocks are intentionally narrower:

```dsql
post_stats: posts | aggregate {
  count
  latest_post: max .created_at
} |
```

General pipelines should remain a separate consideration and should not be
introduced through `where` clauses or other clause positions.

## Possible Shape

```dsql
query RevenueByMonth {
  orders.pipeline {
    filter status == "paid"
    group {
      month: date_trunc("month", created_at)
    }
    aggregate {
      total: sum(amount)
      count: count()
    }
    sort month desc
  }
}
```

This is inspired by relational pipeline languages. It is not part of the query
language and should not be added until normal selection syntax proves awkward
for real use cases.

Open questions:

- Whether this belongs in dsql or should be generated from another layer.
- How pipeline output shape composes with nested selections.
- Whether pipeline syntax can share aggregate and grouping semantics.
- How provider-specific functions are represented.
