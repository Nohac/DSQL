# Drive TypeScript dsql callsite transforms from compiler metadata

**ID:** d447cd36 | **Status:** Open | **Created:** 2026-06-14T17:55:07+02:00

## Summary

Move TypeScript `dsql(...)` callsite discovery and rewrite metadata toward the
compiler/daemon output so the Vite plugin can become a thin source-edit adapter.

## Context

The Vite plugin currently uses regular expressions to find simple
`const Name = dsql\`...\`` and `const Name = dsql(\`...\`)` bindings and then
imports the generated operation/fragment handle. The TypeScript runtime also
has a small regex fallback for untransformed `dsql(...)` calls.

The compiler already extracts embedded DSQL regions and source maps. It should
be possible to expose enough metadata to tell Vite which source ranges are
rewriteable, which generated export they correspond to, and which query barrel
to import from. That would remove most language guessing from the plugin and
make unsupported forms explicit diagnostics.

## Done When

- Compiler/daemon metadata includes embedded TypeScript DSQL callsite ranges,
  definition names/kinds, and source-file ownership.
- Vite rewrites from compiler-provided metadata instead of regex-discovering
  DSQL bindings.
- Unsupported embedded forms produce diagnostics rather than silent
  non-transforms where practical.
- The runtime fallback is either removed or clearly limited to non-executable
  placeholder behavior with tests.
