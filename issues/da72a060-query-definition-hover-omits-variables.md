# Query definition hover omits variables

**ID:** da72a060 | **Status:** Done | **Created:** 2026-07-16T21:49:51+02:00

Query-definition hover is missing the query's inferred variables, which were
included by the POC.

```dsql
query MovieDetailPageQuery {
  title(where .id == $$movieId limit 1) {
    ...HeroPanelFields
    episode_nr
  }
}
```

Expected: hovering `MovieDetailPageQuery` shows the query signature or input
section including `$$movieId` and its inferred type.

Actual: hover only displays `query MovieDetailPageQuery`.

Resolved by publishing a tracked, demand-gated variable aggregate for each
definition and joining it into definition hover by `NodeKey`. Query hovers now
render the POC-compatible nested variable shape, including an explicit message
for queries without variables, while sessions without variable demand retain
the lightweight definition hover.
