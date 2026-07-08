# Language specifications

These specs describe the dsql language surface: queries, variables,
directives, pagination, policies, mutations, and the rest. They were carried
over from the proof-of-concept repository and describe the language design —
which the rewrite preserves — not the compiler internals.

Two caveats when reading:

- **Implementation status varies.** The core query language (queries,
  fragments, selections, clauses, scoped predicates, variables, planning,
  SQL generation, formatting) is implemented; several specs here (mutations,
  aggregates, policies, split-fetch, resolution scopes, computed
  expressions, ...) are design documents for features that are not built
  yet in this repository.
- **Architecture references are superseded.** Where a spec mentions
  compiler machinery, `docs/architecture/compiler.md` is authoritative for
  this codebase.
