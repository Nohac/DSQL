# Diagnose unmapped column types instead of failing at execution

**ID:** efa43eb8 | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

`DataType::from_database_type` maps 18 PostgreSQL type names and returns
`Unknown` for everything else, including `date`, `timestamp` without time zone,
`interval`, `bytea`, `inet`, arrays, ranges, domains, and native enums. `Unknown`
then accepts every literal kind, so a predicate comparing such a column against
an incompatible literal type-checks clean.

When the comparison operand is a variable rather than a literal, the operation
compiles, generates SQL, and generates host types, and only fails when the
executor reaches `bind_scalar` and returns `UnsupportedType`. The TypeScript
runtime validator fails the same input at the same point. The compiler already
knows the column type is unmapped and should say so.

Report unmapped column types where they are used, at compile time. Selecting such
a column may remain permitted; binding a parameter against one must not compile
silently.

Acceptance criteria:

- A new diagnostic code reports a predicate, order, or dynamic-input operand
  resolved to an unmapped column type, with the database type name in the message.
- `Unknown` stops accepting arbitrary literal kinds without a diagnostic.
- Selecting an unmapped column remains allowed and is covered by a test.
- No compiled operation reaches `ExecuteError::UnsupportedType` for a type the
  compiler could see; execution tests assert the diagnostic fires first.
