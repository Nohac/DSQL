# Split project crate lib into focused modules

**ID:** 541bb102 | **Status:** Open | **Created:** 2026-06-14T17:55:07+02:00

## Summary

Split `crates/project/src/lib.rs` into focused modules and keep `lib.rs` as the
public re-export surface.

## Context

The project crate has accumulated configuration, project loading, schema
loading, resolution map handling, and related helpers in a single `lib.rs`.
That makes ownership boundaries harder to see and increases the chance that new
project functionality is added in the wrong layer.

## Done When

- `lib.rs` primarily declares modules and re-exports public project APIs.
- Configuration parsing, schema/catalog loading, document discovery, and
  resolution scope handling live in focused module files.
- Public API compatibility is preserved where the current crate expects it.
- Existing `dsql-project` tests pass without broad behavior changes.
