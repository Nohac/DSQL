# Fix optional predicates with nullable operands

**ID:** 6578e784 | **Status:** Done | **Created:** 2026-07-22T18:52:41+02:00

Nullable predicate operands are only hoisted when they appear as a bare
parameter in the planner's preferred path-on-the-left shape. In a query that
declares `$$value?` in its definition header, a reversed legal body expression
such as `$$value == .id` first turns the variable operand into an `Optional`
filter, then embeds that boolean guard inside a scalar comparison. This produces
invalid PostgreSQL for non-boolean values and incorrect predicate semantics for
boolean values. An expression with two nullable operands also tracks only the
first one.

Hoist optionality around the complete predicate atom regardless of operand
order, and represent every nullable operand that controls structural absence.
Preserve the existing `and`/`or`/`not` pruning algebra.

Acceptance criteria:

- Path/variable and variable/path comparisons produce equivalent plans and SQL.
- Every nullable operand of one predicate atom participates in pruning.
- Null under `or` selects the other branch and never lowers to unconditional
  truth.
- Plan, SQL, and live PostgreSQL regression tests cover reversed operands and
  multiple nullable operands.
