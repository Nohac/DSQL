# Preserve comments in definition headers and fragment bindings

**ID:** 62ae0413 | **Status:** Open | **Created:** 2026-07-22T18:52:43+02:00

The formatter rebuilds definition headers from refinement/filter nodes and
fragment binding lists from binding items, omitting comment nodes between those
items. Formatting can therefore delete source comments despite the formatter's
round-trip preservation contract.

Include comments in source order when formatting both constructs, following the
existing comment-aware patterns elsewhere in the CST formatter.

Acceptance criteria:

- Leading, trailing, and between-item comments in definition headers survive.
- Comments in fragment binding lists survive.
- Formatting remains idempotent.
- Formatter snapshots cover line and block comments around multiline items.
