# Implement explicit fragment composition

**ID:** abae32e5 | **Status:** Open | **Created:** 2026-09-14T19:04:24+02:00

Implement `...(A and B)` and `...(A or B)` so independently authored fragments
can explicitly combine compatible overlapping selections and query predicates.

The [spec](../docs/spec/fragment-composition.md) owns the language contract; the
[plan](../docs/plans/fragment-composition.md) maps it to the existing compiler.
Preserve operand input identity, enforced policies, and grouping. Compose each
operand's complete paginated window by proven row identity and deterministic
left-priority order. Reject unsupported source/access/transform overlaps.
Row order must remain stable across generations for unchanged query semantics,
inputs, effective catalog, and database contents.
Recompute composed result shapes and version the metadata contract before
updating TypeScript consumers.

Done when conformance cases C01–C17 pass through core checks/SQL, live execution,
generation, TypeScript, and LSP/daemon edits, with ordinary spread collision
behavior preserved outside explicit groups. Specification and planning are
complete; implementation has not started.
