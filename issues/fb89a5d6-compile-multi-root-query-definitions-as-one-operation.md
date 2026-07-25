# Compile multi-root query definitions as one operation

**ID:** fb89a5d6 | **Status:** Done | **Created:** 2026-07-24T23:09:24+02:00

A query definition with multiple root selections is accepted by the language,
but generation currently emits one independently named artifact per root. For
example, a definition named `FeaturedMovieQuery` with `featured_aggregate` and
`featured` roots produces:

- `FeaturedMovieQuery_featured_aggregate_0`
- `FeaturedMovieQuery_featured_1`

The daemon callsite still targets the source definition's stable identity,
`<scope>/operation/FeaturedMovieQuery`. Because that artifact is absent, the
TypeScript renderer's target lookup returns `undefined` and crashes while
reading `.metadata`.

Compile every query definition into one operation artifact named after the
definition. Its single PostgreSQL statement and result metadata must include
all root selections. Do not move composition into the TypeScript renderer or
silently execute roots as unrelated client-side operations.

Acceptance criteria:

- A multi-root query publishes exactly one operation artifact under the query
  definition's name.
- SQL returns every root result in one row and preserves source root aliases.
- Parameters, defaults, variants, trusted context, policies, and fragment
  provenance are merged deterministically without duplication or path drift.
- The operation result shape contains every root, including mixed aggregate,
  singular, flattened, and collection roots.
- Daemon callsite targets always resolve to an artifact in the same compile
  result; invariant failures produce a deliberate protocol/compiler error, not
  a JavaScript `TypeError`.
- Compiler, generation, daemon, TypeScript rendering, and live execution tests
  cover a mixed aggregate and ordinary-root embedded query.
