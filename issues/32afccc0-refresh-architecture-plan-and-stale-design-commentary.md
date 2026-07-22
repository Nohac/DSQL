# Refresh architecture plan and stale design commentary

**ID:** 32afccc0 | **Status:** Open | **Created:** 2026-07-22T18:56:40+02:00

Repository guidance names `docs/plan.md` as the intended-design source of
truth, while that document describes itself as a historical port record and
does not cover shipped policies/filters, variable defaults, fragment
containment/lifting, or the generated/effective catalog direction. Nearby code
comments also claim occurrence/span ordering where merged contracts are now
path-lexicographic.

Reconcile the architecture-document ownership story and refresh the canonical
design documentation without turning the historical phase record into a second
implementation manual.

Acceptance criteria:

- The documented source-of-truth hierarchy is unambiguous.
- Current architecture documentation covers policies/filters, definition-level
  defaults and fragment lifting, and generated versus effective catalogs.
- Historical phase records remain clearly labeled as historical.
- Stale ordering comments in `entities/variable.rs` and
  `dsql-generate/src/assemble.rs` describe the actual ordering contract.
