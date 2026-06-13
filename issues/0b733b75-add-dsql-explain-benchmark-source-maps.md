# Add DSQL explain benchmark source maps

**ID:** 0b733b75 | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Add source-map support for DSQL explain/benchmark reports so expensive
PostgreSQL plan nodes can be mapped back to DSQL source spans.

## Context

The underlying tool should generate SQL, optionally run
`EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`, and emit a structured report. The
report should include compiler provenance from DSQL spans to generated SQL
aliases, relation scans, joins, filters, order clauses, limits, and selected
fields.

The LSP can later consume stored or on-demand reports through commands,
diagnostics, inlay hints, or code lenses. `EXPLAIN ANALYZE` should not run
automatically on edits because it executes the query and can be expensive.

Useful report hints include actual time, loops, rows, rows removed by filter,
scan type, index used, sort temp buffers, shared hit/read buffers, and repeated
lateral loop counts. Missing-index style hints should be evidence-based.

## Done When

- Generated SQL has enough source-map metadata to relate plan nodes back to
  DSQL ranges.
- A benchmark/explain command can emit a structured report.
- LSP integration is designed around explicit user-triggered analysis.
- Snapshot tests cover generated SQL source maps and sample JSON plans mapped
  back to DSQL ranges.
