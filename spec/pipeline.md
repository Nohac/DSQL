# Pipeline Queries

Status: consideration.

Some grouping and reporting workflows may fit a relational pipeline syntax
better than nested selections.

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

