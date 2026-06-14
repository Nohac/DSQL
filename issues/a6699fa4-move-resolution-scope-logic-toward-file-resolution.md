# Move resolution scope logic toward file resolution

**ID:** a6699fa4 | **Status:** Open | **Created:** 2026-06-14T17:55:00+02:00

## Summary

Evaluate whether resolution scopes can be modeled primarily as file ownership
and document selection, so lower compiler/generation stages receive ordinary
definition sets instead of carrying scope mechanics throughout the pipeline.

## Context

Scoped generation currently has explicit scope handling across document loading,
definition grouping, import resolution, artifact building, and TypeScript render
metadata. That works, but the amount of scope-specific logic suggests the
boundary may be too high-level.

An alternative is a file resolution layer that decides which documents belong to
which generated surface and passes the correct local/imported definitions into
the existing resolver/check/plan pipeline.

## Done When

- The current scope responsibilities are documented by layer.
- A design sketch identifies which responsibilities can move into file
  resolution and which must remain in semantic resolution.
- The generation pipeline has fewer scope-specific branches without losing
  duplicate/import diagnostics.
- Tests continue to cover default scope, independent scopes, imported shared
  definitions, and source-file-to-scope metadata.
