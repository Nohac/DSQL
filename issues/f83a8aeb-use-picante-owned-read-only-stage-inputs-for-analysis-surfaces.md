# Use Picante-owned read-only stage inputs for analysis surfaces

**ID:** f83a8aeb | **Status:** Open | **Created:** 2026-06-14T19:44:24+02:00

## Summary

Make Picante/the frontend analysis database the top-level entrypoint for parser,
lowering, checking, linting, planning, and generation-facing analysis, while
keeping the individual compiler stages oblivious to Picante.

## Context

Compiler stages should stay pure and accept explicit inputs by reference. The
current project/generation path still rebuilds enough source context that
presentation layers are tempted to recover line/column information later from
files or ad hoc source copies.

The desired shape is for Picante-owned queries to materialize shared read-only
stage inputs such as source snapshots/Ropes, extracted definitions, scoped
definition collections, fragment maps, catalogs, and options. Stages should
consume those borrowed inputs directly instead of receiving Picante database
handles, `Arc<Rope>` plumbing, or presentation-layer file reads.

This should also give structured diagnostics a single place to derive host-file
byte ranges and line/column positions before daemon, CLI, Vite, or LSP
formatting.

## Done When

- Frontend/Picante exposes read-only stage input objects that can be borrowed by
  parse/lower/check/lint/plan/generation-facing analysis.
- Compiler stages remain free of Picante database handles and continue to accept
  explicit pure inputs.
- Generation and daemon diagnostic transport can derive host-file byte ranges
  and line/column positions from shared source snapshots/Ropes, without reading
  files in TypeScript or reconstructing source text at presentation boundaries.
- Existing CLI, LSP, generation, and TypeScript diagnostics tests continue to
  pass.
