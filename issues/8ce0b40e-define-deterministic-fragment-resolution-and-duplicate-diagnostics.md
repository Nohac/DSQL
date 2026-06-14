# Define deterministic fragment resolution and duplicate diagnostics

**ID:** 8ce0b40e | **Status:** Done | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Make fragment resolution deterministic and remove silent overwrites. The
immediate implementation should work for the current default project scope and
leave a clear path for config-defined resolution maps.

## Context

`FragmentMap` is keyed by fragment name and insertion replaces any previous
fragment with the same name. With multiple open files or multiple project
documents, duplicate fragment names can become order-dependent and the earlier
definition can be silently lost.

Same-file duplicate checks do not fully solve the cross-file/project case. DSQL
should not add user-visible namespace syntax such as `api::FragmentName`.
Instead, fragment lookup should be bound to a resolver scope selected by project
configuration. Before multiple configured scopes exist, the default scope should
still detect duplicates deterministically instead of overwriting.

Future config-defined resolution maps will allow separate surfaces, such as
frontend and API DSQL, to use the same fragment names independently. This issue
is the lower-level resolver correctness needed for that model.

The current frontend resolver path also deduplicates fragments while building a
`FragmentMap`, so the implementation needs duplicate-preserving data structures
and diagnostics at every resolver construction boundary, not only a different
`insert` implementation.

All validation and generation entrypoints should flow through the same compiler
pipeline. Duplicate-fragment diagnostics should therefore be produced by shared
analysis/resolution code and consumed by LSP, CLI validation, daemon generation,
and environment-run generation rather than reimplemented per entrypoint.

## Done When

- The fragment resolution model is documented in code/tests.
- `FragmentMap` no longer silently overwrites an existing fragment without
  preserving enough information to report duplicates.
- Duplicate fragment names across files in the same resolver scope are
  deterministic diagnostics.
- Duplicate diagnostics are emitted from a clear analysis stage and are surfaced
  consistently in LSP diagnostics, CLI validation, and generation validation.
- The internal resolver can be scoped without requiring user-visible qualified
  fragment syntax.
- LSP, CLI, planning, checking, linting, and generation all use the same lookup
  semantics.
- Regression tests cover duplicate fragments in two files and duplicate
  fragments in embedded regions.
