# Avoid SQL template sentinel collisions

**ID:** 5bf53cda | **Status:** Open | **Created:** 2026-07-17T14:25:17+02:00

PostgreSQL rendering temporarily substitutes large integer sentinels for
parameterized `limit`/`offset` values and exact numeric literals, then replaces
their decimal text in the formatted SQL. A user-written integer equal to an
allocated sentinel can therefore be replaced accidentally when both occur in
one query.

Replace the global text substitution with a collision-proof placeholder path,
or allocate sentinels only after proving the chosen digits do not occur in the
rendered statement. Add a regression containing a literal in the sentinel
range beside parameterized pagination and an exact decimal predicate.
