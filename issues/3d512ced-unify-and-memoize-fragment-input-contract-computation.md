# Unify and memoize fragment input contract computation

**ID:** 3d512ced | **Status:** Open | **Created:** 2026-07-22T18:52:43+02:00

Variable inference computes local definition bindings twice, recursively
recomputes nested fragment contracts per spread site without memoization, and
the planner re-derives effective variables instead of consuming the published
`DefinitionVariables` fact. Contract binding and value routing also classify
spread binding lists independently, allowing error cases to diverge.

Introduce one canonical per-definition contract computation and one per-root
spread decision model, such as `Contained`, `LiftWhole`, or `BindLeaves`.
Publish the result as tracked bowl facts consumed by planning, generation, and
services, and memoize recursive fragment contracts within an evaluation.

Acceptance criteria:

- Local definitions are inferred once per effective contract computation.
- Contract bindings and value rewrites derive from the same validated spread
  decisions.
- Planning consumes the published effective contract instead of re-walking.
- Nested repeated spreads do not cause exponential contract recomputation.
- Incremental and nested-spread regression tests preserve current semantics.
