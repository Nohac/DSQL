# Use numeric DSQL defaults for bigint

**ID:** 1e8d4231 | **Status:** Open | **Created:** 2026-08-04T22:15:42+02:00

Logical `bigint` expressions accept DSQL number literals, but declaration
defaults currently require quoted decimal strings. This leaks the JavaScript-
safe wire representation into source syntax and makes `bigint` the only
numeric logical type whose default syntax is a string.

Replace the source contract with numeric integer literals. Preserve the exact
source lexeme while validating it against the signed 64-bit range. Host-
supplied `big_integer` wire inputs remain signed decimal strings, and emitted
metadata defaults remain exact signed decimal strings after compiler-owned
conversion. This source-only replacement does not change manifest version 4.
Remove support for quoted `bigint` defaults rather than retaining a
compatibility form.

Acceptance criteria:

- `docs/spec/operation-execution.md` distinguishes numeric DSQL declaration
  defaults from string wire inputs and metadata defaults.
- A `bigint` input declaration accepts unquoted zero and both signed 64-bit
  limits, while out-of-range and non-integer values are diagnostics.
- Quoted source defaults are rejected.
- Collection defaults apply the same rule to each member and reject null
  members.
- Metadata serializes accepted defaults as exact signed decimal strings, and
  maintained Rust and TypeScript runtime paths validate and bind them without
  precision loss.
- Compiler, metadata, TypeScript generation, and runtime integration tests pin
  the complete source-to-wire behavior.
