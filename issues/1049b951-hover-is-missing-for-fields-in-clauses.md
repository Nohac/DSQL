# Hover is missing for fields in clauses

**ID:** 1049b951 | **Status:** Done | **Created:** 2026-07-16T21:49:51+02:00

Hover does not resolve fields referenced inside clause sections. For example,
hovering `info` in the `order by` clause below returns no result:

```dsql
movie_info_idx(
  where .info_type_id == 101
    and .title.kind_id == 1
    and .title.movie_info_idx.info_type_id == 100
  order by info desc, id asc
           ^
  limit 16
)
```

Expected: field hovers in `where`, `order by`, and other clause expressions use
the same resolved catalog information as selection-field hovers.

Actual: the hover request at `info` returns nothing.

Resolved by contributing clause hover candidates directly from
`ResolvedClause` relation, terminal-column, and order-item spans. Selection and
clause hovers now share the same catalog description helpers.
