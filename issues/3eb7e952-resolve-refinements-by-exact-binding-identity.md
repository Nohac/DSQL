# Resolve refinements by exact binding identity

**ID:** 3eb7e952 | **Status:** Open | **Created:** 2026-07-22T18:52:42+02:00

Header refinements currently match either a binding name or the final segment
of any binding path. A caller's `$$limit` can consequently also match contained
or namespaced fragment copies ending in `.limit`, including non-refinable
inherited contracts. Common fragment input names then create spurious
ambiguity or "inherits a fragment root contract" errors.

Resolve refinements against canonical binding identities. Top-level refinements
must target the exact `params.<name>` binding. Structured refinements keep their
specified ambiguity rules, but unrelated contained contracts must not become
candidates. Non-refinable bindings should only produce the inherited-contract
diagnostic when they are the sole exact target.

Acceptance criteria:

- Local and contained/lifted fragment inputs may share leaf names.
- Top-level refinement matching is exact rather than suffix-based.
- Structured ambiguity remains diagnostic and lists the real candidates.
- Checks snapshots cover local, contained, lifted, and namespaced collisions.
