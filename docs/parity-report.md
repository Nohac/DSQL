# POC parity report — 2026-07-16

The ground-up bowl port now covers every actionable behavior found in the
`dsql-poc` audit. Where the current project intentionally differs, the reason
is either a correctness improvement or an explicit owner-level design choice.

## Language and compiler

- The grammar, CST lowering, resolver, checks, lints, variables, planning,
  PostgreSQL rendering, formatting, hover, completion, and source definitions
  are ported into demand-gated bowl systems.
- Diagnostics include the POC's duplicate anonymous-variable binding check.
  The CLI arms the same editor/check demands, so `check` and `validate` cannot
  silently omit those diagnostics.
- Resolution scopes use one transitive, diamond-deduplicating import closure
  for fragment lookup, collision checks, variable inference, planning, and
  generated scope groups. Unknown and cyclic imports fail project loading;
  local/imported and multi-provider query collisions are diagnostics.
- Go-to-definition handles both source definitions and introspected catalog
  tables/columns. Catalog targets resolve into the schema YAML loaded into the
  bowl, preserving the POC editor behavior without adapter-side semantic
  resolution.
- Embedded source ownership is configuration-driven. Resolution entries name
  a resolver and paths; the built-in `dsql` resolver owns whole documents,
  while embedding resolvers currently extract regions by configured regex.
  File extensions have no compiler-defined meaning, leaving room for a future
  tree-sitter extraction provider.

## Project, build, and consumers

- CLI parity includes init, introspect, validate, check, fmt, parse, generate,
  metadata-schema, and metadata-typescript commands.
- Metadata v2, transactional content-addressed publication, immutable
  generations, daemon reconciliation, and the TypeScript renderer/Vite binding
  go beyond the POC. Callsite expression ranges come from the Rust extractor;
  semantic analysis supplies one opaque operation-or-fragment target, and the
  consumer verifies the host SHA-256 before rewriting without rediscovering
  source ownership or definition kinds.
- LSP format-on-save is region-granular and driven by derived DSQL regions, not
  host file extensions. Open buffers, diagnostics, navigation, and residency
  remain bowl-owned.
- Porridge content-revisit and bound-join replanning fixes are pinned. Both
  plain-document and embedded-host A-to-B-to-A regressions remain covered, and
  the daemon no longer needs its reload-on-revisit workaround.

## Verification parity

The integration suite snapshots compiler and editor behavior. The deterministic
`tests/observatory` project supplies the opt-in live PostgreSQL boundary for
operation execution, supported wire types, variants, filters, aggregates,
composite relations, catalog comments, views, and materialized views.

Set `DSQL_OBSERVATORY_DATABASE_URL` to its dynamically allocated URL to run the
live tests. Without it they skip cleanly, so ordinary workspace tests remain
hermetic.

## Intentional differences and remaining decisions

- `@dsql.include_if` is recognized and type-checked but remains an error because
  the POC's empty planner implementation emitted silently unconditional SQL.
  Enabling it requires an owner-approved conditional-selection design.
- Embedded expressions require exactly one top-level definition. Query and
  fragment expressions both rewrite to generated typed handles; empty and
  multi-definition expressions are rejected as ambiguous.
- `@dsql.deprecated` is accepted without artifact metadata, matching the POC.
- The LSP is a dedicated binary, and project-aware single-file analysis replaces
  the POC's hardcoded-catalog fallback.
- CLI `fmt` still refuses embedding hosts; editor formatting safely edits their
  individual DSQL content regions.

There are no accidental POC-parity blockers left. The remaining semantic item
above is deliberately blocked on product design rather than missing
implementation.
Other performance and engine follow-ups remain tracked in `docs/issues.md`.
