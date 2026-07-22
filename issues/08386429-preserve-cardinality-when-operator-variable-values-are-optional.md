# Preserve cardinality when operator-variable values are optional

**ID:** 08386429 | **Status:** Open | **Created:** 2026-07-22T18:52:42+02:00

At-most-one proofs re-derive the refinement name for anonymous variables from
the compared column. For an operator-variable expression such as
`where .id $$operator[==] $$`, inference names the anonymous value input
`params.value`, but cardinality checks look for a refinement named `id`.
Declaring `$$value? = null` can therefore prune the equality at runtime while
the result remains typed and planned as singular.

Use the same resolved binding identity for variable inference, cardinality
proofs, planning, and metadata instead of independently reconstructing names.

Acceptance criteria:

- A nullable anonymous value paired with an operator variable cannot prove a
  unique predicate.
- Non-null equality-only operator variants retain the existing singular proof.
- Cardinality snapshots cover named and anonymous values in both operand
  orders.
