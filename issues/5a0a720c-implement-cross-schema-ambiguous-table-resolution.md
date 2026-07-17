# Implement cross-schema ambiguous table resolution

**ID:** 5a0a720c | **Status:** Open | **Created:** 2026-07-17T01:46:36+02:00

`docs/spec/query.md` says an unqualified root table is valid only when its
name is unique across visible schemas and otherwise produces an
`AmbiguousTable` diagnostic listing qualified candidates. Catalog lookup
currently checks only `default_schema`, so `TableResolution::Ambiguous` is
unreachable for roots and fragment targets.

Implement the specified lookup policy without changing qualified references.
Add catalog and end-to-end diagnostic tests covering a unique non-default
table, an ambiguous name in multiple schemas, and explicit `schema::table`
disambiguation. Wire the existing `imdb-duplicate-relation-path.dsql` fixture
into the ambiguity coverage instead of leaving it orphaned.
