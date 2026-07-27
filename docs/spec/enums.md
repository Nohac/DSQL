# Enumerated Types

Status: consideration.

Enumerated types are nominal logical types backed by a finite, catalog-known set
of variants. dsql should support both native database enums and configured
table- or view-backed enums without imposing provider-specific table-shape
rules on the query language.

This feature is designed but not implemented. It is not a prerequisite for the
current scalar query surface.

## Terminology

An enum has five distinct pieces of information:

- **type identity**: the stable nominal type, such as `OrderStatus`;
- **variant**: the stable value used in dsql source and generated APIs, such as
  `"shipped"`;
- **backing value**: the value stored in the database, such as `20` or a UUID;
- **label**: optional human-facing display text, such as `"Shipped"`;
- **description**: optional documentation for the type or one variant.

The variant and backing value may be identical. They are separate concepts so
a stable generated API does not need to expose storage identifiers.

A dsql enum is a nominal catalog type. It is unrelated to the JSON Schema
`enum` keyword used when describing directive arguments and other JSON values.

## Normalized Catalog Model

Native and table-backed enums normalize into the same conceptual catalog fact:

```text
CatalogEnum {
    identity
    description?
    backing_type
    variants: [
        {
            variant
            backing_value
            label?
            description?
            order
        }
    ]
    provenance
}
```

Enum identity must not collapse to an identity-free `enum` scalar kind. Two
enums containing the same variants remain different logical types. The catalog
therefore needs a representation equivalent to `LogicalType::Enum(EnumId)`, not
a bare `DataType::Enum`.

The generated catalog records provider facts and captured values. Project
configuration and catalog overlays contribute the authored intent needed to
produce the effective catalog described by
[Catalog Overlays](catalog-overlays.md).
All compiler consumers receive that one effective catalog and do not repeat
enum-source resolution.

## Native PostgreSQL Enums

PostgreSQL introspection discovers native enum types, their schema-qualified
identity, variant labels, and database order from the provider catalog. The
variant and database binding are both the native enum label.

`COMMENT ON TYPE` supplies the optional enum type description. PostgreSQL does
not provide a direct comment for an individual enum label, so native variants
normally have no per-variant descriptions. Column comments remain attached to
the columns that use the enum.

Native PostgreSQL enum types are distinct even when their labels are identical.
Type checking must preserve this distinction, including comparisons, defaults,
fragment lifting, and generated input metadata.

## Configured Table-Backed Enums

Ordinary tables and views do not intrinsically declare that their rows form a
closed, migration-managed value set. Projects opt into that interpretation in
`dsql/dsql.toml` so introspection knows which rows to capture.

```toml
[[catalog.enums]]
name = "OrderStatus"
source = "public.order_status"
variant = "code"
value = "id"
label = "label"
description = "description"
order = "display_order"
```

- `name` is the stable nominal enum identity.
- `source` is a schema-qualified table, view, or materialized view.
- `variant` identifies the stable source/API representation and is required.
- `value` identifies the database representation. When omitted, it is the same
  column as `variant`.
- `label` optionally identifies human-facing display text.
- `description` optionally identifies per-variant documentation.
- `order` optionally identifies semantic ordering.

The source object remains an ordinary selectable catalog object. Columns not
used by the enum declaration remain available to queries and relationships.
The enum declaration does not restrict the object to a prescribed number or
shape of columns.

Configuration is deliberately structural. It cannot contain SQL, predicates,
transforms, or provider expressions. A project that needs a filtered or
transformed value set defines a database view and uses that view as `source`.
Open scalar types with structural wire, literal, pattern, and operator
semantics use [`[[catalog.types]]`](catalog-metadata.md#project-type-declarations)
instead. Scalar declarations cannot claim a closed variant set, and enum
declarations cannot widen scalar operator capabilities.

The source object's database comment supplies the enum type description. The
configured description column supplies per-variant descriptions. Labels are
presentation hints for generators, not a localization system and not part of
enum identity.

## Generated Snapshots

`dsql introspect` reads configured enum sources and stores their current values
in the generated catalog. The authored configuration does not duplicate those
values. This keeps ownership explicit:

```text
database migrations own rows
        -> introspection owns the generated snapshot
        -> configuration owns the enum projection
        -> the effective catalog owns compiler semantics
```

The exact YAML representation is an implementation detail until this feature
is implemented. It must nevertheless retain enough information to verify that
the snapshot was captured for the current source, variant, value, label,
description, and order configuration. A missing or structurally stale snapshot
is a project-load error that instructs the user to run `dsql introspect`.

Offline compilation cannot detect rows changed after the last introspection.
Declaring a table-backed enum is therefore an explicit project assertion that
the rows are a closed set managed together with schema migrations. Deployment
and CI should refresh or check introspection after migrations. A future
`dsql introspect --check` mode may automate that freshness check.

## Validation

A configured enum is valid only when:

- its nominal identity is unique in the effective catalog;
- its source and every configured column exist;
- the variant column is non-null, textual, and unique;
- the backing value column is non-null and unique;
- each variant maps to exactly one backing value and vice versa;
- the backing type has a lossless parameter and result representation;
- configured labels and descriptions are textual;
- a configured order is non-null and deterministically orders every variant;
- at least one variant exists; and
- the value count is below a documented compiler safety limit.

Database constraints are preferred uniqueness evidence. Views and materialized
views can use the overlay `assert_unique` assertion once catalog overlays
support it; ordinary tables cannot. Unlike relationship-cardinality inference,
configured enum introspection captures the values, so snapshot validation must
still reject actual nulls or duplicates rather than trusting declared evidence
blindly.

Variants are strings, but they are not restricted to GraphQL identifier syntax.
They remain case-sensitive and preserve whitespace. The dsql string-literal
rules determine how they are written in source.

If no explicit order column exists, introspection canonicalizes snapshot output
by variant for reproducible files. That canonical file order does not grant the
enum semantic ordering operations.

## Propagation To Columns

The configured backing value column has the enum's logical type. Columns with a
physical foreign key to that column inherit the same logical enum identity while
retaining their own nullability.

```text
order_status.id:   OrderStatus
orders.status_id:  OrderStatus
events.status_id:  OrderStatus?
```

A manual relationship proves that two objects can be joined; it does not prove
that one column's values are restricted to an enum. Manual relationships do not
therefore propagate enum typing. Explicitly attaching an enum to a column that
has no physical foreign key is deferred and must be an unmistakable closed-set
assertion when introduced.

## Query Semantics

Enums require no new literal syntax. Context supplies the nominal type:

```dsql
query Orders($status = "pending") {
  orders(where .status_id == $status) {
    id
    status_id
  }
}
```

For a table-backed mapping where `"pending"` maps to `10`, the SQL parameter is
bound using the backing value and backing database type:

```sql
WHERE orders.status_id = $1
```

```text
$1 = 10::smallint
```

The compiler never interpolates either representation into SQL text.

Enum typing applies consistently to:

- literals and definition-level defaults;
- public and trusted-context variables;
- nullable comparison pruning;
- membership collections;
- fragment containment, lifting, and remapping;
- completion and hover; and
- generated validators and API types.

An unknown literal variant is a compile-time diagnostic. An unknown dynamic
variant is rejected by generated input validation before database execution.

## Symmetric Runtime Representation

Variants are the observable DSQL and generated-API representation in both
directions:

- inputs translate from variant to backing value;
- database results translate from backing value to variant.

This symmetry prevents a field from accepting `"pending"` while unexpectedly
returning `10` or a UUID. An unknown backing value is a runtime contract error;
it must not leak through as an untyped scalar or silently become null.

The implementation may perform result conversion in generated SQL or in the
server-side result decoder. The choice is not observable and should be made
according to query shape and provider capabilities. Backing-value maps belong
in server-only operation data. Browser-safe metadata needs variants, labels,
descriptions, and ordering, but does not need storage identifiers.

## Operators And Ordering

Equality, inequality, and membership compare enum identity and translate through
the backing representation. Enums with different identities are incompatible
even when their variants or backing types happen to match. Text operators such
as `like` do not apply merely because variants are written as strings.

Native PostgreSQL enums use their provider-defined ordering. A table-backed
enum has semantic ordering only when `order` is configured. Without semantic
ordering, comparative predicates, `order by`, and order-dependent aggregates
such as `min` and `max` are diagnostics rather than accidental operations on
UUIDs, integer identifiers, or lexical variant order.

Implementing ordered table-backed enums may require a join, correlated lookup,
or bounded generated expression. Equality and membership support should not be
blocked on that later planning work. Grouping and equality-based distinctness
use logical variants; `count` does not require semantic ordering.

## Editor And Generated Metadata

Completion should offer known variants in literals, defaults, membership lists,
and bounded inputs whose expected type is the enum. Hover should show the
nominal type, its description, and the selected variant's label or description
when one is known.

Generated metadata must preserve:

- nominal enum identity;
- nullability and collection shape at each use;
- ordered variants;
- labels and descriptions;
- input/output conversion requirements; and
- source provenance sufficient for diagnostics and navigation.

For TypeScript, the default representation is a string-literal union over
variants:

```ts
type OrderStatus = "pending" | "shipped" | "delivered";
```

Labels and descriptions can additionally drive generated selects, filters,
tables, documentation, and other metaprogramming consumers.

## Implementation Sequence

This feature is intentionally lower priority than catalog overlays and general
relationship metadata. A safe implementation can proceed incrementally:

1. introduce nominal logical types and introspect native PostgreSQL enums;
2. add same-column table-backed enums where variant and backing value match;
3. add distinct variant-to-backing mappings with symmetric runtime conversion;
4. add labels, descriptions, and generator metadata;
5. add semantically ordered table-backed enums; and
6. consider explicit FK-less attachments and native per-variant documentation
   overlays only after concrete use cases require them.

Each phase must update checking, variables, plans, SQL binding, execution,
metadata, editor services, and generation together. Partial enum awareness that
degrades to an ordinary scalar in one stage is invalid.

## Open Questions

- The exact generated YAML representation and compatibility/versioning rules.
- The fixed safety limit for captured variants.
- Whether mapped result conversion is generally cheaper in SQL or in each
  server adapter.
- How provider packages expose enum-like types that are neither native enums
  nor relational value sets.
- Whether native enum variants eventually need project-authored descriptions.
- What explicit syntax should attach a table-backed enum to a column without a
  physical foreign key.
