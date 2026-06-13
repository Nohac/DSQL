# Define top-level definition limits for embedded dsql calls

**ID:** ed1e0bac | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Define and enforce how many top-level DSQL definitions an embedded
`dsql(...)` call may contain.

## Context

Embedded TypeScript documents can contain DSQL inside `dsql(...)` calls or
tagged templates. The behavior is unclear when one embedded call contains
multiple top-level statements or declarations.

Generation, LSP mapping, and generated TypeScript inference are easier to reason
about if an embedded call has a clear contract. A likely rule is to allow only a
single top-level query or fragment per embedded `dsql(...)` expression, but this
needs an explicit decision and diagnostics.

## Done When

- The allowed top-level shape for embedded `dsql(...)` calls is documented.
- Violations produce diagnostics with ranges mapped to the host TypeScript file.
- Generation follows the same rule as LSP diagnostics.
- Tests cover one query, one fragment, multiple definitions, and empty embedded
  content.
