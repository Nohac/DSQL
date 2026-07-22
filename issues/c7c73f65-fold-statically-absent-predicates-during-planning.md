# Fold statically absent predicates during planning

**ID:** c7c73f65 | **Status:** Open | **Created:** 2026-07-22T18:52:43+02:00

Predicates fixed absent by compile-time fragment defaults can survive planning
as permanent optional-guard scaffolding such as `NOT (FALSE) OR TRUE`. This
inflates every generated statement and leaves simplification to PostgreSQL even
though structural absence is already known.

Fold `Absent` and compile-time boolean values through predicate connectives at
plan construction while preserving the specified `and`/`or`/`not` pruning
algebra.

Acceptance criteria:

- A statically absent atom emits no optional guard.
- A completely absent `where` emits no predicate clause.
- Mixed `and`, `or`, and `not` cases retain the same runtime meaning.
- Plan and SQL snapshots demonstrate the reduced output.
