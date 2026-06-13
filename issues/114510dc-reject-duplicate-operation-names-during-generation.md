# Reject duplicate operation names during generation

**ID:** 114510dc | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Generation should reject duplicate operation names before writing artifacts or
emitting generated TypeScript.

## Context

Lowering reports duplicate query names within one parsed file, and the CLI query
loader handles multiple definitions for a single query lookup. Generation,
however, builds operations from all project documents and should validate that
the final operation artifact names are unique across the whole project.

Duplicate names can come from repeated query names across files, or from
multi-root query output naming rules that derive the same operation name.

## Done When

- Generation validates unique operation artifact names across all loaded
  documents.
- Duplicate names produce a clear error with source context for the conflicting
  definitions.
- Validation mode reports the same issue without writing artifacts.
- LSP/project diagnostics expose the same duplicate-name problem where practical.
- Regression tests cover duplicates in one file and duplicates across files.
