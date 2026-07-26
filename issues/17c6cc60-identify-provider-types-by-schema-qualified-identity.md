# Identify provider types by schema-qualified identity

**ID:** 17c6cc60 | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

`type_map.yaml` does not identify a PostgreSQL type. `TypeMetadata.schema` is
populated from `operator_schema`, which `PG_TYPE_INTROSPECTION_QUERY` defines as
`pg_proc.pronamespace` — the namespace of a function implementing an operator on
the type, not `pg_type.typnamespace`. `validate_column_types` compares that field
as if it were type identity. The value is also whichever operator sorted first,
so it is unstable across unrelated database changes.

The same query inner-joins `pg_operator`, so a type with no operator taking it as
the left operand never reaches `type_map.yaml` at all. `provider_type_map`
additionally keys on bare `typname`, so two types with the same name in different
schemas collapse into one ambiguous entry. Both cases surface as the overlay
error "column provider type identity is absent or ambiguous; rerun dsql
introspect", which is a false report whose suggested remedy changes nothing.

Select types from `pg_type` directly, carry `typnamespace` as the type's own
schema, aggregate operators in a separate pass, and key the provider type map on
the schema-qualified identity.

Acceptance criteria:

- `TypeMetadata` carries the type's own namespace, not an operator's namespace.
- Types with no left-operand operator appear in `type_map.yaml` with an empty
  operation set.
- Same-named types in different schemas remain distinct entries.
- An overlay relationship across two identically named types in different schemas
  is rejected as a type mismatch rather than as ambiguous identity.
- Introspection unit tests and `type_map.yaml` fixtures cover both cases.
