# Disambiguate repeated flattened relation aggregates in SQL

**ID:** e2c5ca99 | **Status:** Done | **Created:** 2026-07-17T17:12:10+02:00

Two flattened aggregate selections of the same relation under one parent are
accepted when their contributed output keys differ, but SQL generation assigns
both lateral joins the same alias and reads both fields through that alias.

```dsql
fragment MovieSignals on title {
  ...movie_info_idx(where .info_type_id == 101) | aggregate {
    rating: max .info
  }
  ...movie_info_idx(where .info_type_id == 100) | aggregate {
    votes: max .info
  }
}
```

The generated SQL repeats aliases such as
`movie_info_idx_json_3a6c0768` for both lateral joins. This is invalid
PostgreSQL and also makes the parent projection unable to distinguish the two
aggregate instances.

Alias identity must include the semantic selection instance, not only the
resolved relation/output path. Add SQL snapshot and live execution coverage
for repeated filtered flattened aggregates over the same relation, including
use through an imported fragment. The result must expose independently
filtered `rating` and `votes` scalar fields without changing metadata paths.
