# dsql Specification

This directory contains the tracked dsql language specification.

The current focus is the read-query language: documents, queries, selection
sets, catalog resolution, fragments, aliases, and core clauses.

The draft notes in `local-docs/spec` are useful background, but they are not
authoritative. They contain older syntax experiments and examples that may not
match the current design direction.

## Documents

- [Query Language](query.md)
- [Scoped Predicates](scoped-predicates.md)
- [Variables](variables.md)
- [Directives](directives.md)
- [Relationship Naming](relationship-naming.md)
- [Metadata Sources](metadata-sources.md)
- [JSON Schema Overrides](json-schema-overrides.md)
- [Aggregates](aggregates.md)
- [Grouping](grouping.md)
- [Pagination](pagination.md)
- [Split Fetch](split-fetch.md)
- [Policies And Permissions](policies.md)
- [Code Generation Metadata](codegen.md)
- [Query Primitives](query-primitives.md)
- [Inline Fragments](inline-fragments.md)
- [Computed Expressions](computed-expressions.md)
- [Mutations](mutations.md)
- [Pipeline Queries](pipeline.md)

## Statuses

- `in progress`: actively shaping the language direction.
- `unfinished`: likely feature area, syntax and behavior still incomplete.
- `consideration`: captured idea, not yet accepted as a feature direction.
- `RFC`: larger design proposal that needs deeper review before implementation.

## Design Rules

- dsql is a domain specific query language for describing result shape and
  generating SQL and application artifacts.
- Query bodies should look SQL-like where applicable.
- Catalog resolution is schema-aware. Unqualified table names resolve through
  the project default schema, which is `public` unless configured otherwise.
- Relation names come from catalog relationship metadata and should not be
  singularized, pluralized, or otherwise rewritten by the language layer.

## Draft Reconciliation

The local drafts contain useful future directions, but this tracked first pass
intentionally narrows and normalizes the query language:

- Advanced features are treated as future layers unless they change the core
  model.
- Examples assume relation names are catalog names, not inferred singular or
  plural variants.
- Examples assume schema-qualified references are valid and that unqualified
  table references resolve through the configured default schema.
