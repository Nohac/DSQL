# Backfill variable binding diagnostics and runtime edge coverage

**ID:** c2415f76 | **Status:** Open | **Created:** 2026-07-22T18:52:43+02:00

Variable defaults and fragment lifting have strong happy-path snapshots, but
several implemented diagnostics and runtime matrix rows have no direct coverage.
This makes binding compatibility, optional pruning, and materialization changes
risky to refactor.

Add focused integration coverage for:

- plain nullable refinements and nullable refinements with non-null defaults;
- single-root `$` and `$$` lifting, shorthand forwarding, and multilevel lifts;
- mixed whole-root/leaf, duplicate, ambiguous, and missing target bindings;
- duplicate refinements and compatible/incompatible merged contracts;
- filter-assignment nullability and collection default shapes;
- nullable membership, offset, bare boolean, and reversed predicate operands;
- contained deep defaults and every materialization error class; and
- live PostgreSQL execution of the highest-risk `or` pruning combinations.

Prefer compact table-driven integration tests and shared fixtures over one test
per syntax example. Snapshot user-visible diagnostics, plans, SQL, and protocol
responses.
