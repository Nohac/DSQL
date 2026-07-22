# Normalize DSQL cache keys from materialized inputs

**ID:** 0bd09277 | **Status:** Open | **Created:** 2026-07-22T18:52:42+02:00

TypeScript cache-key builders currently embed the caller's raw variables.
Omission, explicitly passing a declared default, and nullable dynamic inputs in
their canonical empty form can therefore create different keys for the same
executed operation. The browser operation object currently lacks enough input
metadata to materialize before building the key.

Choose and implement one client-safe materialization contract for operation
keys. Browser-safe operation data may carry defaults and dynamic-shape metadata,
but must not expose SQL, trusted context values, or other server-only payloads.

Acceptance criteria:

- Omitted and explicitly supplied defaults produce equal keys.
- Nullable bounded dynamic predicate/order values normalize to their canonical
  empty object/array identity.
- Context-dependent operations still require an explicit context scope.
- Runtime and package-level tests pin key equivalence and server/browser data
  separation.
