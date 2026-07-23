# Language specifications

These specs describe the dsql language surface: queries, variables,
directives, pagination, filters and access rules, mutations, and the rest. They
were carried over from the proof-of-concept repository and describe the
language design — which the rewrite preserves — not the compiler internals.

Two caveats when reading:

- **Implementation status varies.** The core query language (queries,
  fragments, selections, clauses, scoped predicates, variables, planning,
  SQL generation, formatting), aggregates, filters/access rules, definition
  defaults and fragment input lifting, and resolution scopes are implemented.
  Deferred extensions are called out in their individual specs. Several other
  specs here (mutations, split-fetch, computed expressions, enumerated types,
  [catalog overlays](catalog-overlays.md), ...) are design documents for
  features that are not built yet in this repository.
- **Architecture references are superseded.** Where a spec mentions
  compiler machinery, `docs/architecture/compiler.md` is authoritative for
  this codebase.
- **Tooling contracts also live here.** For example,
  [TypeScript Distribution And Project Wiring](typescript-distribution.md)
  specifies package and scaffold boundaries rather than DSQL syntax.
