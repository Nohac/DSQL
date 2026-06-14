# Include source locations in Vite generation diagnostics

**ID:** 221d13e8 | **Status:** Done | **Created:** 2026-06-14T18:22:40+02:00

## Summary

Vite generation errors should include the source file and line/column for each
diagnostic, not only the diagnostic kind and byte range.

## Context

When the Vite plugin asks the DSQL daemon to compile/generate and diagnostics
contain errors, the thrown error currently looks like:

```text
Error: cannot generate while diagnostics contain errors

Check OutputKeyTooLong 30..134: selection output key `...` is 104 bytes; PostgreSQL result aliases must be at most 63 bytes
```

The byte range is not actionable in Vite's terminal output because it omits the
originating source file and line/column. The daemon/generation transport already
has enough information to include the source file and source offset for embedded
DSQL regions; the TypeScript daemon client should preserve and format that
location when surfacing generation failures.

This should include a root-cause pass before changing formatting. Specifically,
verify whether the DSQL daemon already sends enough structured diagnostic data
for Vite to render file/line/column messages, or whether the daemon currently
collapses diagnostics into strings before the TypeScript client sees them. The
fix should avoid papering over missing metadata in Vite if the real loss happens
earlier in the compile/generate transport.

Also revisit the Vite-facing error formatting as a whole. The current thrown
`Error` is technically useful for failing startup, but the terminal output
should be optimized for users locating and fixing DSQL source errors, not for
debugging the daemon client stack.

## Done When

- Vite startup/generation failures print diagnostics with file path, line, and
  column, for example `src/file.tsx:12:5`.
- Embedded DSQL diagnostics account for source offsets so locations point at
  the host file, not just the embedded snippet.
- Diagnostics still include kind/code, severity, byte range, and message for
  tooling that wants structured details.
- The implementation documents where diagnostic location metadata is produced,
  transported, and formatted, so future Vite/daemon changes do not regress back
  to byte-range-only messages.
- A regression test covers a generated diagnostic such as `OutputKeyTooLong`
  flowing through the daemon/Vite error path with a human-readable location.
