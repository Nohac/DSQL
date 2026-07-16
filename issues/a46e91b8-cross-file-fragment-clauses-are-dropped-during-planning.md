# Cross-file fragment clauses are dropped during planning

**ID:** a46e91b8 | **Status:** Done | **Created:** 2026-07-16T21:54:14+02:00

Planning a query that spreads a fragment from another file drops clause parts
that depend on semantic resolution. Literal `limit` values survive, but
fragment-owned `where` predicates and `order by` items disappear from the
generated SQL.

The imdsql `TopRatedMoviesPanelQuery` exposes this through
`RankedMovieFields`, defined in `queries/shared/title-fragments.dsql`:

```dsql
ratings: movie_info_idx(
  where .info_type_id == 101
  order by id asc
  limit 1
) {
  info
}
```

Generated SQL currently keeps only the relation join and `limit 1`. The live
page consequently displays arbitrary `movie_info_idx.info` values as ratings
and votes. For example:

| Title | Displayed rating/votes source | Expected rating | Expected votes |
| --- | --- | --- | --- |
| Cow Dog | `.....3.113` | `9.9` | `6` |
| A Date with FEAR (The Making) | `........28` | `9.8` | `5` |

The list itself is intentionally ordered by the root rating row (`info desc,
id asc`), not alphabetically. The corrupt labels come from the expanded
fragment selections.

Likely cause: `plan_queries` indexes `ResolvedClause` paths and order items only
when their `BelongsToFile` matches the query file. `SpreadExpansion` then walks
fields and clauses from the fragment file, whose resolved spans are absent from
those indexes. Filter planning returns no predicate and order planning filters
out every unresolved item, while `limit` needs no resolution lookup and
therefore remains.

Expected: cross-file and same-file fragment expansion preserve identical
selection clauses in generated plans and SQL. Add an integration snapshot that
spreads a fragment containing nested relation `where`, `order by`, and `limit`
clauses from a second source file.

Resolved by indexing semantic clause resolutions by their owning clause entity
in planning and variable inference, with cross-file SQL and variable snapshots.
