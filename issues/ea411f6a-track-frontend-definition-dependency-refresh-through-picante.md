# Track frontend definition dependency refresh through Picante

**ID:** ea411f6a | **Status:** Open | **Created:** 2026-06-14T17:55:07+02:00

## Summary

Evaluate moving frontend definition dependency refresh work into Picante-tracked
queries so expensive dependency recomputation can be memoized and invalidated by
input identity.

## Context

The frontend database currently refreshes definition dependencies by walking
query and fragment inputs and rebuilding dependency state. If this work becomes
expensive, it should be tracked by Picante rather than manually cached outside
the query system.

## Done When

- The cost and invalidation behavior of dependency refresh is documented.
- Dependency refresh inputs are represented by Picante-tracked queries where it
  materially reduces recomputation.
- Frontend analysis still produces deterministic diagnostics for duplicate and
  missing definitions.
- Tests cover dependency updates after query and fragment edits.
