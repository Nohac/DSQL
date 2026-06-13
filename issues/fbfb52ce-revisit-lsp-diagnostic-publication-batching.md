# Revisit LSP diagnostic publication batching

**ID:** fbfb52ce | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Revisit diagnostic publication batching for the LSP once larger workspaces make
the current all-open-documents publication strategy too expensive.

## Context

After an open document changes, the server publishes diagnostics for all open
documents. This keeps cross-file fragment diagnostics simple and lets the
analysis/input tracking decide what actually recomputes.

That MVP behavior is acceptable while the number of open documents is small,
but may become noisy or slow for larger workspaces. Future work can add debounce,
batching, or cancellation without making fragment-specific invalidation rules in
the LSP layer.

## Done When

- Diagnostic publication remains generic and not special-cased for fragments.
- Large edit bursts avoid unnecessary client traffic.
- Expensive publication work can be debounced, batched, or cancelled.
- Behavior is covered by LSP-facing tests or a documented performance check.
