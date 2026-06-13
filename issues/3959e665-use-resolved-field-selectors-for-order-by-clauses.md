# Use resolved field selectors for order by clauses

**ID:** 3959e665 | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

`order by` clauses should use the same resolved field-selector semantics as the
rest of analysis and SQL planning.

## Context

Order-by validation currently checks the order item as a direct field on the
current table. This risks diverging from predicate/path resolution and from the
selector semantics users expect elsewhere in DSQL.

The fix should make order-by resolution explicit and precise, including
qualified relation names if those are supported for order fields. Diagnostics
should point at the unresolved selector.

## Done When

- Order-by fields are resolved through the same semantic selector model used by
  checking/planning for comparable field references.
- Invalid order-by fields report precise diagnostics.
- SQL generation uses the resolved field rather than reinterpreting text later.
- Focused tests cover direct columns, invalid selectors, and any supported
  qualified relation form.
