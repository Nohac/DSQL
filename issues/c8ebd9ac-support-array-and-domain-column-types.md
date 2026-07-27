# Support array and domain column types

**ID:** c8ebd9ac | **Status:** Done | **Created:** 2026-07-26T11:48:49+02:00

Arrays and domains are common in ordinary schemas and both currently resolve to
`Unknown`. A `text[]` column introspects as `_text`, which matches no entry in
the type-name table. A domain introspects under its own name, so a domain over
`text` loses every capability its base type has.

Both are mechanical once the catalog holds type identity, and both are cheaper
than the nominal enum support that Enumerated Types (docs/spec/enums.md) defers.

Domains collapse to their base type through `typbasetype` while retaining their
own name for display, and inherit the base type's capabilities. Arrays gain an
element-typed logical form through `typelem` and must be distinguished from the
existing `collection` flag on input fields, which describes an input's shape
rather than a column's type.

Sequenced after 1c36c801.

Acceptance criteria:

- A domain column behaves as its base type for operators, literals, binding, and
  generated host types, while hover and generated documentation show the domain
  name.
- An array column is selectable with an element-typed host type rather than
  `unknown`.
- Array and scalar collection inputs remain distinguishable in generated
  metadata; a scalar input bound to an array column is a diagnostic.
- Catalog, plan, SQL, and metadata snapshots cover one domain column and one
  array column.

## Resolution

Catalog metadata now requires an explicit scalar, domain, or array structure.
Domain bases and array elements are schema-qualified type identities resolved
through a validated, cycle-safe catalog graph. Domains inherit scalar semantics
while retaining their declared provider identity; database arrays remain
distinct result shapes and cannot bind through scalar input paths.

Manifest version 5 replaces result-field scalar aliases with a required
shape-aware value contract. PostgreSQL generation preserves native arrays when
their JSON representation is exact and casts bigint, numeric, and text-cast
element arrays to `text[]` before JSON conversion. TypeScript renders database
arrays recursively, including multidimensional arrays, without conflating them
with relation collections or public input collections.

The Observatory catalog and live operation suite cover text and inet domains,
text and inet arrays, bigint values above JavaScript's safe integer limit, and
a two-dimensional bigint array. Type maps without the required structure are
rejected rather than upgraded.
