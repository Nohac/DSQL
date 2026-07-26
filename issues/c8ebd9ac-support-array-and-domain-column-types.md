# Support array and domain column types

**ID:** c8ebd9ac | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

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
