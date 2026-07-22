# Reject null elements in collection defaults

**ID:** 0ca8dcdb | **Status:** Open | **Created:** 2026-07-22T18:52:42+02:00

Scalar default matching treats `null` as compatible with every logical type,
including elements of collection defaults. Contracts such as
`$$ids = [1, null]` pass checking but cannot be lowered as filter fragments and
fail only during SQL generation.

Reject null collection elements until collection element nullability is
represented explicitly throughout metadata, binding, and SQL generation.

Acceptance criteria:

- Null elements in collection defaults produce a targeted definition-header
  diagnostic.
- A nullable collection itself may still default to `null` according to the
  variable specification.
- Empty and non-null homogeneous collection defaults continue to work.
- Checks, metadata, execution, and SQL tests cover the distinction.
