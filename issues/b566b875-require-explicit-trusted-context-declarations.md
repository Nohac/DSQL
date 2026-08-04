# Require explicit trusted-context declarations

**ID:** b566b875 | **Status:** Open | **Created:** 2026-08-04T22:15:42+02:00

Trusted context is currently inferred from each `$:name` use. A misspelling
therefore creates another inferred input instead of identifying an unknown
trusted value, and no authoritative declaration supplies nominal catalog types
for enums or extension-defined values.

Implement the mandatory scope-level `context { name: type }` declarations
specified in `docs/spec/variables.md`. Context entries use normal effective
scope visibility and collision rules. Their declared types, including context
collections, are the sole source of context typing. Remove declaration-by-use,
project-configuration declarations, provider-metadata declarations, and every
fallback that accepts an undeclared context name.

Context remains operation-global rather than liftable, as settled by completed
issue `120ca946`. A fragment, filter, or condition contributes its context uses
to the consuming operation without remapping them.

Acceptance criteria:

- The compiler and manually maintained editor grammar parse and format context
  blocks, including qualified catalog/provider types and context collections.
- Lowering and the effective scope resolver diagnose duplicate, imported, and
  ambiguous context entry names using the existing definition collision rules.
- Every context use resolves to one declaration and validates scalar or
  collection shape, boolean roles, and nominal enum/provider-type identity.
- Undeclared context names are errors; no inference or configuration/provider
  fallback remains.
- Context declarations are required, non-null, and have no DSQL-authored
  defaults.
- Completion lists visible declarations, hover reports the declared contract,
  and goto-definition lands on local and imported declaration entries.
- Generated operation metadata contains only declarations used by the
  operation's effective query, fragment, filter, and condition closure, and
  runtime validation rejects missing or invalid values.
- Integration snapshots cover parsing, formatting, scope imports and
  collisions, checks, services, metadata, and runtime execution.
