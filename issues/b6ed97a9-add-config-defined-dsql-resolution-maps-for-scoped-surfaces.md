# Add config defined DSQL resolution maps for scoped surfaces

**ID:** b6ed97a9 | **Status:** Done | **Created:** 2026-06-13T14:23:24+02:00

## Summary

Add config-defined DSQL resolution maps so one project can have multiple
independent DSQL surfaces, such as frontend queries and API queries, while
sharing the same schema and optional shared definitions.

## Context

Some projects need separate groups of DSQL documents that target different
runtime surfaces. For example, frontend DSQL and API DSQL may both use the same
database schema and policies, but should be able to define a fragment or query
with the same name without overriding each other.

This should be resolution scoping, not language-level namespacing. DSQL source
should continue to use plain names:

```dsql
query MoviePage {
  title {
    ...MovieFields
  }
}
```

There should be no syntax such as `api::MovieFields`. The active resolution map
comes from project configuration and document ownership.

Potential config shape:

```toml
[resolution.shared]
documents = ["queries/shared/**/*.dsql"]

[resolution.frontend]
documents = ["src/**/*.tsx", "queries/frontend/**/*.dsql"]
imports = ["shared"]

[resolution.api]
documents = ["queries/api/**/*.dsql"]
imports = ["shared"]
```

Schema/catalog configuration remains project-global. Shared maps are imported
into another map's effective resolver by value. Imported definitions are emitted
into the importing generated surface when needed; generated outputs should not
depend on a separate shared output by default.

Rust owns resolution-map assignment and effective resolver construction. Vite,
the user generator entrypoint, and adapter templates should receive scope
metadata from the compiler/daemon and use it to choose which generators or
layouts to run; they should not independently decide resolution ownership.

## Design Constraints

- Resolution maps are internal/project configuration, not DSQL syntax.
- Fragment and query lookup uses plain names in the current effective map.
- Same names can exist in different maps without conflict.
- Duplicate names inside one effective map are diagnostics.
- Local definitions that collide with imported definitions should be errors at
  first; do not implement shadowing until there is a concrete need.
- If two imported maps provide the same name to one effective map, that is also
  a diagnostic.
- Imported shared definitions are copied into each generated surface by default
  to keep outputs self-contained and deployment boundaries simple.
- Policies and future rule definitions should be able to follow the same import
  model as fragments and queries.

## Done When

- Project config can define named resolution maps with document globs and
  imports.
- Existing projects behave as a single default resolution map when no maps are
  configured.
- Each loaded document has a deterministic owning resolution map.
- The compiler/daemon exposes enough scope metadata for Vite transforms and
  TypeScript generators to choose the correct generated surface.
- Checking, planning, linting, generation, completion, hover, and definition use
  the current map's effective resolver.
- The same fragment/query name can exist in `frontend` and `api` without
  conflict.
- Duplicate names inside one map or introduced by imports produce stable
  diagnostics.
- Shared/imported definitions resolve by plain name and are emitted into each
  importing surface's generated artifacts.
- Tests cover default-map compatibility, frontend/api duplicate names, imported
  shared fragments, local/import collisions, and duplicate names from two
  imports.
