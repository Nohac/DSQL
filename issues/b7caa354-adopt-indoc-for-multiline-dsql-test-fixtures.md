# Adopt indoc for multiline DSQL test fixtures

**ID:** b7caa354 | **Status:** Done | **Created:** 2026-07-21T21:36:08+02:00

Multiline inline DSQL fixtures are commonly assembled with `concat!` and one
quoted string per source line. This is noisy, obscures the query's visual
structure, and makes fixtures harder to edit and review.

Adopt `indoc!` for multiline DSQL fixtures so test sources can use natural
indentation. Apply the migration consistently, preferably as a dedicated
mechanical cleanup, and avoid changing fixture contents or snapshots beyond
formatting-equivalent source spans unless a test intentionally depends on
offsets.
