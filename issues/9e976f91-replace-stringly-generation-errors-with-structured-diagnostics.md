# Replace stringly generation errors with structured diagnostics

**ID:** 9e976f91 | **Status:** Open | **Created:** 2026-06-14T17:55:00+02:00

## Summary

Replace ad hoc `String` generation errors with structured error/diagnostic
types that preserve category, source location when available, severity, and
stable user-facing messages.

## Context

The generation pipeline currently accumulates some failures in `Vec<String>` by
calling `to_string()` or pushing hand-written strings such as anonymous-query
generation errors. That makes these failures harder to surface consistently in
CLI, daemon, LSP, and tests, and it obscures which errors can be converted into
source diagnostics.

A general pass should identify similar stringly error accumulation patterns in
generation/project-facing code and replace them with typed errors or transport
diagnostics.

## Done When

- Generation validation errors use typed enums or diagnostic structs rather than
  raw strings.
- Errors with source locations are emitted as normal diagnostics.
- Non-source generation failures use structured error types such as thiserror
  enums.
- Tests assert diagnostic codes/messages without depending on incidental string
  construction.
