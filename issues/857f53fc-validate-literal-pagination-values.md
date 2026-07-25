# Validate literal pagination values

**ID:** 857f53fc | **Status:** Done | **Created:** 2026-07-22T21:21:48+02:00

Definition defaults reject negative and out-of-range `limit` and `offset`
values, but query-authored numeric literals have no equivalent semantic check.
Planning substitutes zero when a literal cannot be parsed as `u64`, while a
literal between `i64::MAX` and `u64::MAX` can reach PostgreSQL and fail as an
out-of-range bigint.

Validate literal pagination values against the same non-negative `i64` domain
as definition defaults. Emit a targeted check diagnostic and retain the
planner's non-widening fallback for invalid programs that are planned despite
diagnostics.

Acceptance criteria:

- Negative and greater-than-`i64::MAX` literal limits and offsets diagnose.
- Zero and `i64::MAX` remain valid.
- Invalid literal limits cannot broaden a query if planning is forced.
- Checks and SQL snapshots cover the boundaries.
