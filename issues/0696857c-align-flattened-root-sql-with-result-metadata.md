# Align flattened root SQL with result metadata

**ID:** 0696857c | **Status:** Done | **Created:** 2026-07-17T16:45:00+02:00

A flattened root selection publishes its child fields directly in result
metadata, but PostgreSQL rendering still aliases the JSON object with the
source selection name:

```dsql
query Metrics {
  ...title | aggregate { title_count: count }
}
```

The generated TypeScript result is `{ title_count: number }`, while SQL returns
one row shaped like `{ title: { title_count: number } }`. Executors such as the
IMDb TanStack Start example return `rows[0]` unchanged, making the static and
runtime contracts disagree. Nested flattened selections work because their
fields are merged into the parent JSON object.

Define one canonical root-flatten execution shape and make SQL, metadata, and
consumer execution agree. Add an end-to-end renderer/runtime regression that
executes a root flattened singular selection and a root flattened aggregate
against PostgreSQL and asserts the actual JSON keys match the generated type.

Resolution note: ordinary root table selections are collection-valued and
cannot flatten. Coverage therefore pairs the root flattened aggregate with an
ordinary root containing a catalog-proven singular flattened relation.
