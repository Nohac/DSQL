# Introduce a catalog type arena addressed by TypeId

**ID:** 1274316f | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

Column types are held twice and neither holding is authoritative. `Column.database_type`
is a bare PostgreSQL type name preserved for display, and `Column.data_type` is a
closed nine-variant enum every consumer actually reads. Type facts are therefore
duplicated per column, cannot carry provider identity, and have nowhere to record
per-type capabilities.

Give the effective catalog a dense type arena addressed by `TypeId`, populated
from `type_map.yaml`, and make `Column` reference a type rather than restate one.
Enumerated Types (docs/spec/enums.md) already requires this shape: a nominal type
identity rather than an identity-free scalar kind.

This step is a refactor only. `DataType` remains the public logical type and keeps
its current variants; consumers reach it through the arena instead of a field.
Behaviour, generated metadata, and the schema YAML format are unchanged.

Sequenced before b50babe0.

Acceptance criteria:

- `Catalog` owns a `types` arena and `Column` addresses it by `TypeId`.
- Every existing `DataType` consumer resolves through the arena with no behaviour
  change.
- `semantic_fingerprint` hashes type identity rather than the per-column
  `database_type` string and `data_type` enum.
- Schema YAML, generated operation metadata, and all existing snapshots are
  byte-identical.
