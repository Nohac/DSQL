# Support nested variable expansion in generated TypeScript

**ID:** dc823a87 | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Generated TypeScript should correctly expand variables inside nested input
structures.

## Context

DSQL variable inference can produce nested paths for selections, clauses,
fragment spreads, and structured input envelopes. Generated TypeScript should
represent those nested variables without collapsing types incorrectly, losing
required fields, or producing unusable shapes.

This should be verified through the actual TypeScript checker, not only emitted
text snapshots, because `never` and overly-wide types can pass superficial
assertions.

## Done When

- Nested variables in generated TypeScript produce the intended input/params
  shapes.
- Required and optional nested envelopes are represented correctly.
- Type-level regression assertions verify the generated types are not `never`.
- Tests include at least one nested relation variable and one nested fragment
  variable.
