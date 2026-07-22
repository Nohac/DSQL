# Complete only legally refinable definition inputs

**ID:** c073fec4 | **Status:** Open | **Created:** 2026-07-22T18:52:43+02:00

Definition-header completion currently offers inherited non-refinable bindings
and inputs already refined in the same header. Accepting those suggestions
produces diagnostics. Completion details also omit the specified semantic role
and source location, and candidate deduplication can hide structured ambiguity.

Build header completions from the effective set of legal refinement targets,
not every inferred binding with a matching sigil.

Acceptance criteria:

- Non-refinable and already-refined inputs are excluded.
- Ambiguous structured candidates remain visible and explain the ambiguity.
- Completion detail includes generated path, type, role, and source location.
- Binding-list cursor positions complete the target definition's inputs instead
  of fragment names.
