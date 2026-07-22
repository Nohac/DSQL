# Language specifications

These specs describe the dsql language surface: queries, variables,
directives, pagination, filters and access rules, mutations, and the rest. They
were carried over from the proof-of-concept repository and describe the
language design — which the rewrite preserves — not the compiler internals.

Two caveats when reading:

- **Implementation status varies.** The core query language (queries,
  fragments, selections, clauses, scoped predicates, variables, planning,
  SQL generation, formatting), aggregates, and resolution scopes are
  implemented. Predicate additions and filter integration are called out as
  pending in their individual specs. Several other specs here (mutations,
  split-fetch, computed expressions, enumerated types, ...) are design
  documents for features that are not built yet in this repository.
- **Architecture references are superseded.** Where a spec mentions
  compiler machinery, `docs/architecture/compiler.md` is authoritative for
  this codebase.
