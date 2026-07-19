# Report one primary diagnostic for unresolved aggregate roots

**ID:** 4d657891 | **Status:** Done | **Created:** 2026-07-19T22:23:36+02:00

A misspelled table used as an aggregate root currently produces three errors
for one unresolved name: semantic and planning `TableNotFound` diagnostics plus
an aggregate-cardinality error derived from the unresolved source.

The semantic check owns table resolution failures. Planning should skip roots
that cannot resolve, and aggregate checking should not infer cardinality from
an unresolved target. Preserve cardinality diagnostics for resolved singular
relations and scalar columns.

Resolved by making semantic checks the sole owner of unresolved query-root
diagnostics. Planning skips unresolved roots, while aggregate resolution avoids
deriving cardinality errors from an unresolved target. The regression snapshot
now contains one `Check TableNotFound` diagnostic.
