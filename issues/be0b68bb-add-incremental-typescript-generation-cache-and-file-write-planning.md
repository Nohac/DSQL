# Add incremental TypeScript generation cache and file write planning

**ID:** be0b68bb | **Status:** Open | **Created:** 2026-06-13T19:50:32+02:00

## Summary

Add an incremental generation cache and write planner for TypeScript generation
so hot reloads and repeated generation runs only rewrite files whose rendered
contents actually changed.

## Context

The planned TypeScript output splits generated code into many per-definition
files. That improves bundler behavior, hot reload granularity, and git diffs,
but only if generation does not blindly rewrite every file on every change.

Rust should not own TypeScript output paths. Rust should compile and cache DSQL
build metadata/artifacts, grouped by resolution scope when scopes exist. The
TypeScript generator owns filesystem layout and should use artifact hashes plus
rendered content hashes to decide which generated files to write, keep, or
remove.

The existing Rust metadata/build folder is the right conceptual place for DSQL
compiler artifacts. A new cache layer may also be needed on the TypeScript side
for rendered file planning, because the generator decides the output paths.

## Desired Behavior

- Editing one query should rewrite only that query's generated files and any
  affected barrels/manifests.
- Editing one shared fragment should rewrite each importing scope's affected
  files, but not unrelated definitions.
- Re-running generation with identical inputs should avoid touching file mtimes.
- Removed definitions should remove stale generated files that were previously
  owned by DSQL.
- Hot reload should see small, targeted file changes instead of a full generated
  directory rewrite.

## Design Constraints

- Rust owns DSQL build artifacts and scope metadata, not TypeScript output
  layout.
- TypeScript renderers own output paths and therefore must participate in file
  write planning.
- The cache key should include the DSQL artifact hash, renderer version/layout
  inputs, scope name, and rendered file content.
- Barrels and indexes are expected to change when definitions are added or
  removed; ordinary definition edits should not rewrite unrelated definition
  files.
- Generated-file cleanup must avoid deleting user files. Only files recorded as
  owned by the DSQL renderer should be removed.
- Vite, CLI, daemon, and environment-run generation should use the same
  generation pipeline and cache semantics.

## Done When

- `renderDsql` and adapter renderers write files through a shared write planner.
- Unchanged rendered files are not rewritten.
- Stale generated files owned by the renderer are removed safely.
- The generated ownership/cache manifest records enough information to plan the
  next run.
- Scope-aware generation uses the same cache/write planner per scope.
- Tests cover no-op generation, single-query edits, fragment edits, definition
  removal, and barrel-only updates.
