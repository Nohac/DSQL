# Preserve trusted context through fragment spreads

**ID:** 120ca946 | **Status:** Open | **Created:** 2026-07-22T18:52:42+02:00

Fragment spread contract binding only processes the structured and top-level
public roots. A trusted `$:context` input used inside a spread fragment remains
referenced by SQL but can be absent from the operation's effective contract and
generated context metadata, causing execution to reject it as an undeclared
parameter.

Trusted context is global rather than liftable. Preserve its binding unchanged
through every spread and merge it into the operation contract with normal type
compatibility checks.

Acceptance criteria:

- A query spreading a fragment that uses `$:key` generates context metadata and
  executes end to end.
- Nested and repeated spreads deduplicate compatible context requirements.
- Incompatible context uses produce a deterministic diagnostic.
- Core, generation, and execution integration tests cover the complete path.
