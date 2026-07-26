# Preserve integer precision in operation results

**ID:** c6c4edf9 | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

Integer inputs are constrained to the JSON-safe domain and rejected outside it
(docs/spec/operation-execution.md). Results have no matching guarantee. `int2`,
`int4`, and `int8` all map to the `int` logical type, and `public_scalar_expression`
casts only `numeric` to text, so an `int8` column value beyond 2^53 crosses
`json_build_object` as a JSON number and silently loses precision in any host
runtime that parses JSON numbers as IEEE-754 doubles.

This is the same failure `numeric` already avoids by crossing the wire as text,
applied to a type the compiler currently cannot distinguish from `int4`.

Decide and specify the result contract for wide integers: either retain `int8`
as a distinct logical type with a text wire encoding, or document that `int`
results are only safe within the JSON-safe domain and reject wider columns at
selection time. Silent truncation is not an acceptable third option.

Acceptance criteria:

- The chosen contract is specified in docs/spec/operation-execution.md alongside
  the existing input domain table.
- An `int8` column holding a value beyond 2^53 either round-trips exactly or
  produces a diagnostic; it does not return a rounded number.
- Generated TypeScript for the affected fields matches the chosen encoding.
- Execution tests cover a value beyond 2^53 in both a selected column and an
  aggregate result.
