# Deduplicate generated wire types with sparse overlays

**ID:** a0aff697 | **Status:** Done | **Created:** 2026-07-28T13:50:57+02:00

Generated operation modules repeated complete result, params, input, and
dynamic-input structures for both host and PostgreSQL wire representations.
Most operations have no representation differences, while mapped operations
usually differ at only a few scalar leaves.

Make host contracts canonical. Identical wire contracts alias the host type;
differing contracts use keyed sparse replacements that rebuild only ancestor
branches leading to representation-changing leaves. Preserve nullability,
requiredness, recursive dynamic predicates, fragment composition, and
database-array shapes.

Resolved by adding `DsqlReplaceFields` and generating sparse wire
projections for operation results, params, inputs, and dynamic inputs.
Operation metadata remains fully expanded for materialization even when the
wire type reuses host fragment composition.
