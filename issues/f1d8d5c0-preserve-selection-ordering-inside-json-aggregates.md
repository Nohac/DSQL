# Preserve selection ordering inside JSON aggregates

**ID:** f1d8d5c0 | **Status:** Done | **Created:** 2026-07-17T17:35:31+02:00

Collection SQL currently orders a derived source subquery and then wraps it in
`JSON_AGG(...)` without an aggregate-local `ORDER BY`. PostgreSQL does not
guarantee that the aggregate consumes rows in the derived-table order, so a
typed result array may not preserve the query selection order.

This is visible in the IMDb example, where ranks are assigned from the returned
array index after:

```dsql
movies: movie_info_idx(order by info desc, id asc limit 16) {
  ...title { id title }
}
```

Render order-sensitive collection aggregation with an explicit ordering inside
`JSON_AGG`, carrying the resolved order expressions through any safety-cap or
limit subquery without changing limit semantics. Add SQL snapshots and a live
execution regression with ties to prove both primary and deterministic
secondary ordering survive JSON construction. Cover nested collections as well
as root collections.
