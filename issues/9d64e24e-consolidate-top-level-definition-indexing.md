# Consolidate top level definition indexing

**ID:** 9d64e24e | **Status:** Open | **Created:** 2026-06-14T17:55:00+02:00

## Summary

Replace scattered query/fragment lookup and artifact plumbing with a shared
top-level definition index keyed by definition identity and backed by an enum
for each definition kind.

## Context

Several paths separately retrieve, store, and iterate queries and fragments:
frontend resolver inputs, generation extraction, scoped generation, and built
artifact grouping. This duplicates ownership rules and will become harder to
maintain when more top-level definitions are added, such as policies, mutations,
or other future definition kinds.

The core model should expose one authoritative top-level definition collection
that can still provide typed views for query-specific and fragment-specific
passes.

## Done When

- A shared top-level definition enum represents queries, fragments, and planned
  future definition kinds.
- Frontend analysis and generation use a single definition index as the source
  of truth for lookup and ordering.
- Scoped generation groups definitions through the shared model instead of
  maintaining independent query/fragment collections where avoidable.
- Existing query and fragment tests continue to pass with deterministic source
  ordering.
