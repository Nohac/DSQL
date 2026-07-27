# Regenerate committed catalog fixtures with provider type facts

**ID:** afc4e9e1 | **Status:** Open | **Created:** 2026-07-27T16:10:25+02:00

The committed IMDb and observatory `type_map.yaml` files predate provider-owned
type capabilities. They intentionally remain provider-less while the
compatibility fallback is established, but maintained catalog fixtures should
eventually represent fresh introspection output.

Until regeneration, those provider-less fixtures also inherit compiler fallback
assumptions that are not valid for every PostgreSQL type. In particular, the
legacy `json` fallback permits equality and ordering even though PostgreSQL
`json` has neither; fresh provider facts correctly disable both.

Regenerate each fixture from its documented PostgreSQL database after the
catalog seed and schema are reproducible. Review the resulting type set rather
than mechanically accepting it: the old type query used inner joins and may
have omitted types without direct operators.

Acceptance criteria:

- committed `type_map.yaml` files carry provider kind, category, ordering, and
  native operation facts;
- column files retain formatted types and modifiers from the same snapshot;
- at least one compatibility fixture remains provider-less, or an equivalent
  focused test continues to cover the fallback path;
- catalog, generation, and live-query fixtures remain green.
