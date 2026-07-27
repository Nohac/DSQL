# Regenerate committed catalog fixtures with provider type facts

**ID:** afc4e9e1 | **Status:** Done | **Created:** 2026-07-27T16:10:25+02:00

The committed IMDb and observatory `type_map.yaml` files predate provider-owned
type capabilities. Maintained catalog fixtures should represent fresh
introspection output rather than exercising the compiler's synthetic
provider-less catalog mode.

Until regeneration, those provider-less fixtures also inherit compiler-owned
assumptions that are not valid for every PostgreSQL type. In particular, the
synthetic `json` capabilities permit equality and ordering even though
PostgreSQL `json` has neither; fresh provider facts correctly disable both.

Regenerate each fixture from its documented PostgreSQL database after the
catalog seed and schema are reproducible. Review the resulting type set rather
than mechanically accepting it: the old type query used inner joins and may
have omitted types without direct operators.

Acceptance criteria:

- committed `type_map.yaml` files carry provider kind, category, ordering, and
  native operation facts;
- column files retain formatted types and modifiers from the same snapshot;
- a focused synthetic catalog test covers compiler-owned behavior without
  presenting it as support for an older metadata format;
- catalog, generation, and live-query fixtures remain green.

## Resolution

The committed IMDb and Observatory catalogs were regenerated from their live
PostgreSQL databases. Type rows now contain provider classification,
comparison, ordering, and structural facts; column rows retain exact formatted
types and modifiers from the same snapshots. The expanded type closure was
reviewed and is intentional: introspection now retains structural dependencies
that the former inner-join query omitted.

A focused provider-less catalog test continues to cover the compiler-owned
synthetic path. Maintained catalogs no longer exercise that path, and incomplete
current type maps fail during parsing or catalog construction.
