# Centralize TypeScript module specifier normalization

**ID:** fd81c8af | **Status:** Open | **Created:** 2026-06-14T17:55:07+02:00

## Summary

Centralize generated TypeScript module specifier construction so renderers do
not duplicate path normalization logic.

## Context

Multiple TypeScript renderers convert absolute or project-relative paths into
import specifiers by calling `relative(...)`, replacing Windows path separators
with `/`, and adding a leading `./` when needed. The logic is correct for ESM
module specifiers but duplicated and visually noisy at call sites.

## Done When

- A shared helper builds normalized ESM import specifiers from `root`,
  `fromFile`, and target module path inputs.
- TanStack Query, TanStack Start, and DSQL-owned renderers use the helper.
- Tests cover relative paths, already-bare module specifiers, project-relative
  render metadata, and Windows-style path separators.
