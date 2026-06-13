# Add stable ordering lint diagnostics

**ID:** 75fbb8bb | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Add configurable lint diagnostics for selections whose limited results may have
unstable ordering.

## Context

Queries with `limit` or `offset` but no deterministic `order by` can return
different rows between executions. DSQL should provide compiler-as-reviewer
guidance for this common query hygiene issue.

Suggested rules:

- `unstable_limit_without_order_by`
- `non_unique_order_by_with_limit`

Default severity should probably be configurable as `off | info | warning |
error`, with `info` as a likely default.

Examples that should lint:

```dsql
query CompanyMovies {
  company_name {
    movie_companies(limit 3) {
      note
    }
  }
}
```

```dsql
query MovieInfo {
  movie_info_idx(order by info desc limit 16) {
    info
  }
}
```

The second rule needs catalog uniqueness metadata and should treat an order-by
chain as deterministic if it includes a unique column or unique column set.

## Done When

- `limit`/`offset` without `order by` produces a configurable lint diagnostic.
- Non-unique ordering with `limit` produces a softer configurable diagnostic
  once the needed uniqueness metadata exists.
- Root and nested selections are both covered.
- Tests cover configured severities and disabled lints.
