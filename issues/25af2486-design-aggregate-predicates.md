# Design aggregate predicates

**ID:** 25af2486 | **Status:** Done | **Created:** 2026-07-17T16:24:34+02:00

Implement the scalar relation-aggregate predicates reserved by the aggregate
specification:

```dsql
users(where (.posts | count) >= $$minimum_posts) { id }
users(where .posts | exists) { id }
```

The transform binds before comparison and must reuse the selection aggregate
functions' types, empty-input behavior, and SQL null semantics. In particular,
comparisons against `min`/`max` of an empty relation follow SQL three-valued
logic and exclude the parent row.

Keep the first increment narrow: no clauses on the relation path, no multi-step
paths, and no general pipe blocks inside clauses. Add parser, resolution,
planning, SQL, variable-inference, diagnostic, and service coverage without
turning pipes into a general predicate pipeline.

Resolved with a closed scalar-aggregate expression over direct collection
relations. Predicate aggregates share selection aggregate function and operand
typing, lower to correlated PostgreSQL scalar subqueries or `EXISTS`, preserve
empty-input NULL semantics, infer typed variables, and participate in editor
hover, tokens, and completion. Broader predicate pipelines remain out of scope.
