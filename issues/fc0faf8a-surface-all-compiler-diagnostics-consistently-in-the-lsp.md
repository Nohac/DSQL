# Surface all compiler diagnostics consistently in the LSP

**ID:** fc0faf8a | **Status:** Open | **Created:** 2026-06-14T17:55:00+02:00

## Summary

Ensure diagnostics produced by shared parsing, definition extraction,
resolution, checking, planning, linting, and generation validation are exposed
through the same analysis surface and therefore appear in LSP diagnostics.

## Context

Some diagnostics currently appear during validation/generation but are not
visible in the editor. Recent examples include duplicate definition diagnostics
preserved for fragment resolution, output key length diagnostics, and
fragment-specific generation diagnostics.

The LSP should not need to know each diagnostic-producing pass individually.
Compiler/frontend analysis should aggregate diagnostics in source order and the
LSP should publish that aggregate for open documents.

## Done When

- Duplicate fragment/definition diagnostics are visible in LSP diagnostics.
- Check diagnostics such as overlong output keys are visible in LSP diagnostics.
- Fragment and query diagnostics are transported through one generic diagnostic
  path instead of special-cased generation-only adapters.
- LSP tests cover at least one diagnostic from each relevant analysis stage.
