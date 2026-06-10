# Investigate deriving diagnostic codes for check diagnostics

**ID:** fe3b3e40 | **Status:** Open | **Created:** 2026-06-10T22:33:44+02:00

## Summary

`CheckDiagnosticKind::code()` manually maps each payload-carrying variant to a
`DiagnosticCode`. This is repetitive and easy to forget when adding a new check
diagnostic kind.

## Context

The mapping may be collapsible with strum variant metadata, another derive
crate, or a small local helper macro. `CircularFragmentSpread` currently maps to
`UnknownFragment`, so the replacement needs to support explicit per-variant
codes rather than assuming variant names always match `DiagnosticCode`.

## Done When

- `CheckDiagnosticKind::code()` no longer contains a large hand-written match.
- The mapping remains explicit for variants whose diagnostic code differs from
  the variant name.
- Existing diagnostic snapshots and focused check tests still pass.
