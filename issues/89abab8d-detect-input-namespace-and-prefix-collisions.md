# Detect input namespace and prefix collisions

**ID:** 89abab8d | **Status:** Done | **Created:** 2026-07-22T18:52:42+02:00

Merged input contracts only diagnose incompatible bindings with exactly equal
paths. A scalar or bounded dynamic input at `params.namespace` can coexist with
lifted leaves such as `params.namespace.limit`, creating a contract that cannot
be represented or materialized consistently.

Validate path-prefix shape compatibility after local and spread contracts are
merged.

Acceptance criteria:

- Scalar-versus-namespace, dynamic-versus-namespace, and incompatible namespace
  shape collisions produce `InvalidFragmentBinding` diagnostics.
- Compatible leaves within one namespace continue to merge.
- Diagnostics identify both conflicting origins.
- Checks snapshots cover structured, top-level, and cross-root namespace
  mappings.
