# Capture native PostgreSQL enum catalog facts

**ID:** 764e4b30 | **Status:** Done | **Created:** 2026-07-29T23:19:15+02:00

PostgreSQL native enums currently enter the effective catalog as anonymous
text-cast scalars. Their schema-qualified provider identity survives, but
introspection drops the type comment and ordered `pg_enum` labels, leaving
later compiler stages no nominal enum contract to consume.

Capture native enum comments and labels in the generated type map, represent
them as an explicit enum shape in the effective type arena, and reject stale
scalar snapshots for provider types whose PostgreSQL kind is `e`. Preserve the
existing text-cast capabilities until the later semantic and generation
slices consume the new facts.

Acceptance criteria:

- introspection records native labels in `enumsortorder` and enum type comments;
- `TypeId`/`TypeKey` remain the nominal identity without a `DataType::Enum`;
- domains and arrays retain structural edges to the one enum definition;
- invalid, empty, configured, stale, or internally inconsistent enum payloads
  fail catalog construction; and
- enum variants participate in the effective catalog fingerprint.

## Resolution

Native PostgreSQL enum comments and ordered labels now survive introspection as
an explicit nominal catalog shape. Catalog construction validates the complete
payload, rejects obsolete scalar snapshots with a re-introspection error, and
keeps the existing text-cast capabilities until query semantics consume the
new facts. Domains and arrays retain structural links to the enum, enum data
participates in catalog fingerprints, and `[[catalog.types]]` cannot remap a
native enum or a domain over one.
