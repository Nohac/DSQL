# Complete filter targets rules and body fields

**ID:** aa8221fc | **Status:** Done | **Created:** 2026-07-19T11:24:03+02:00

Filter and condition authoring lacks semantic completion in three places:

- a structural `on { ... }` target should offer deduplicated catalog column
  names;
- after `.field:`, completion should offer every distinct logical type used by
  catalog columns with that name;
- inside the declaration body, `apply`, `where`, and `field` should be offered,
  and expression paths should complete from the concrete target or the fields
  declared by the structural target.

Completions must follow normal source scope and catalog resolution, remain
deterministic, and avoid offering fields unavailable on every structurally
matched target.

Policy-aware completion now classifies structural field/type positions and
declaration bodies from the CST. Catalog field names and logical types are
deduplicated, body keywords include every legal rule, concrete predicates use
their resolved table, and structural predicates expose exactly their declared
fields.
