# Classify per-type wire encoding and bind unmapped types by name

**ID:** 1c36c801 | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

Parameter binding is a closed match over eight logical type names in the executor
and again in the TypeScript runtime. Any other type reaches
`ExecuteError::UnsupportedType`, so a column PostgreSQL handles perfectly well is
unusable as a query operand. The result direction has the mirror-image gap:
`public_scalar_expression` special-cases `numeric` and passes everything else
through whatever `json_build_object` produces.

Both directions need a per-type rule rather than a per-type branch.

Give each catalog type a wire-encoding classification covering how a value
crosses the JSON boundary and how a parameter is bound. Types with no dedicated
binding bind as text with an explicit generated cast to their schema-qualified
type name, which is the general escape hatch: PostgreSQL performs the input
conversion it already knows how to perform.

This moves validation for such types from the client to the database. Keep
pre-execution validation available for types that declare a pattern (fe7d14cf),
and specify the resulting error shape.

Sequenced after 4b1e4216.

Acceptance criteria:

- Wire encoding is a property of the catalog type, not a branch in the executor.
- A parameter of a type with no dedicated binding is bound as text with a
  generated cast and executes successfully.
- The TypeScript runtime validator agrees with the executor on which inputs are
  accepted; conformance fixtures cover both.
- `date`, `timestamp` without time zone, and `inet` are usable as predicate
  operands end to end, with an execution test per type.
- docs/spec/operation-execution.md documents the encoding classes and where
  validation happens for each.
