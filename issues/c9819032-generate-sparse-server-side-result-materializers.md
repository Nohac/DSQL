# Generate sparse server-side result materializers

**ID:** c9819032 | **Status:** Done | **Created:** 2026-07-28T12:37:49+02:00

TypeScript operation handles currently carry every result field at runtime.
`parseDsqlResult` walks and validates the complete result tree on the client,
even when the project configured no result codec. TanStack Query consequently
caches the wire representation and exposes it through direct cache APIs.

Generate one operation-specific, in-place materializer only when a selected
logical type has a configured parser. The TanStack Start server function owns
the fresh result returned by the database executor, applies that materializer
before returning, and hands the final host value to framework serialization and
the query cache. Operations without result codecs perform no result traversal.

This is a pre-1.0 replacement. Remove the generic result-field contract,
client-side parser, selector cache, and raw-cache behavior rather than retaining
fallbacks.

Acceptance criteria:

- An operation without result codecs emits no materializer and performs no
  result traversal.
- A codec operation mutates the executor-owned result in place and preserves
  the root object, relation object, and relation collection identities.
- Generated traversal visits only paths leading to codec fields, including
  nullable relations, relation collections, and recursive database arrays.
- Parser failures name the logical result path and type and prevent the
  partially materialized value from escaping the server function.
- TanStack server functions return host results; Query caches and direct cache
  APIs expose host types without a generated `select`.
- Codec host values must be serializable by the host binding. TanStack Start
  accepts Seroval-supported values; arbitrary class instances require
  application serialization support or a wire-compatible representation.
- Plain and primitive codec results retain TanStack Query's default structural
  sharing. Identity-bearing codec results may be replaced on every refetch
  unless the application supplies an appropriate `structuralSharing` policy.
- The executor transfers exclusive ownership of its result to DSQL
  materialization. It must not retain aliases that could observe partial
  mutation when a later parser fails.
- Runtime and generated APIs contain no `resultFields`, `parseDsqlResult`, or
  `dsqlResultSelector` compatibility surface.
- The TypeScript specification and README state the ownership and framework
  serialization requirements for custom host values.

Resolved by moving result conversion into optional server-only execution
payload materializers. Generated code walks only codec-bearing branches,
mutates executor-owned results in place, and leaves codec-free operations at a
constant-time root check. TanStack server functions and caches now expose host
results directly; the generic client parser and selector APIs were removed.
