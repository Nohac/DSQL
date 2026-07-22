# Catalog Metadata

Status: in progress.

Catalog metadata is the semantic source of truth for table names, column names,
logical types, nullability, primary keys, unique constraints, indexes, and
relationships. Parsing should preserve source structure; catalog resolution
decides what selections mean.

The **generated catalog** is the provider-derived metadata serialized under the
project's schema directory. PostgreSQL introspection owns those files and may
replace them completely when the database changes; users do not merge authored
customizations into generated files.

The **effective catalog** is the single validated result of applying project
configuration and authored catalog overlays to the generated catalog. Checks,
resolution, planning, SQL generation, editor services, and code generation all
consume the effective catalog. They do not independently consult generated
metadata and overlays.

## Metadata Layout

Generated provider metadata is stored as YAML files under the configured schema
directory.

```text
schema/
  type_map.yaml
  public/
    users.yaml
    posts.yaml
  other_schema/
    widgets.yaml
```

Each table file describes one database object.

```yaml
schema: public
name: memberships
object_type: table
description: Tenant-scoped application memberships.
columns:
  - name: tenant_id
    description: Tenant owning this membership.
    database_type: int4
    data_type: int
    not_null: true
  - name: user_id
    database_type: int4
    data_type: int
    not_null: true
  - name: role
    database_type: text
    data_type: text
    not_null: true

constraints:
  - name: memberships_pkey
    kind: primary_key
    columns: [tenant_id, user_id]

foreign_keys:
  - name: memberships_user_fkey
    columns: [tenant_id, user_id]
    references:
      schema: public
      table: users
      columns: [tenant_id, id]

indexes:
  - name: memberships_user_idx
    columns: [tenant_id, user_id]
    unique: true
```

`type_map.yaml` stores database type metadata separately. Provider-native and
configured table-backed enum types require nominal identities and captured
variants beyond this initial scalar mapping; their contract is specified in
[Enumerated Types](enums.md).

```yaml
types:
  - internal_type: int4
    readable_type: integer
    schema: pg_catalog
    operations: []
```

## Object Metadata

Object metadata identifies a selectable database object.

- `schema`: database schema name.
- `name`: database object name.
- `object_type`: object category, such as `table`, `view`, or another
  provider-supported object kind.
- `description`: optional provider-neutral documentation for the object. For
  PostgreSQL, introspection reads `COMMENT ON TABLE` and the equivalent comments
  on views and materialized views.
- `columns`: ordered column metadata for the object.
- `constraints`: table-level primary-key and unique constraints.
- `foreign_keys`: table-level foreign-key constraints.
- `indexes`: table-level index metadata.

Unqualified table references resolve only when exactly one visible schema
contains an object with that name. If multiple visible schemas contain the same
object name, source must use an explicit schema selector such as
`public::users`.

## Column Metadata

Column metadata describes scalar values only. Constraints belong to the
table-level metadata because primary keys, unique constraints, indexes, and
foreign keys can all span multiple columns.

- `name`: database column name.
- `description`: optional provider-neutral documentation for the column. For
  PostgreSQL, introspection reads `COMMENT ON COLUMN`.
- `database_type`: database-native type name.
- `data_type`: dsql logical type name after type mapping.
- `not_null`: whether the column rejects null values.

Descriptions are preserved in generated schema YAML and exposed by editor hover
and completion. They are documentation only: they do not participate in catalog
identity, resolution, type checking, or generated SQL. Enum type and variant
descriptions follow the separate [Enumerated Types](enums.md) contract.

## Constraints

Table-level constraints describe uniqueness guarantees over an ordered column
set.

```yaml
constraints:
  - name: memberships_pkey
    kind: primary_key
    columns: [tenant_id, user_id]

  - name: memberships_external_id_key
    kind: unique
    columns: [tenant_id, external_id]
```

Supported constraint kinds:

- `primary_key`
- `unique`

Column order must be preserved as reported by the database. It is part of the
constraint identity and is needed when matching composite foreign keys to their
referenced columns.

## Foreign Keys

Foreign-key metadata connects an ordered local column set to an ordered
referenced column set.

```yaml
foreign_keys:
  - name: memberships_user_fkey
    columns: [tenant_id, user_id]
    references:
      schema: public
      table: users
      columns: [tenant_id, id]
```

The `columns` and `references.columns` arrays must have the same length. Column
mapping is positional:

```text
tenant_id -> tenant_id
user_id   -> id
```

Relation names are derived from the target table name unless catalog metadata
provides an explicit relationship name. The language layer does not singularize,
pluralize, or otherwise rewrite relation names.

When multiple foreign-key paths connect the same source and target tables, a
query must disambiguate the path with a relation edge selector, such as
`users->assignee_id`.

For composite foreign keys, the default selector joins the local column names in
foreign-key column order with underscores, such as `tenant_id_user_id`.

## Indexes

Index metadata describes access paths and uniqueness information that may not be
represented as a SQL constraint.

```yaml
indexes:
  - name: memberships_user_idx
    columns: [tenant_id, user_id]
    unique: true
```

Indexes may be used for linting and planning hints. A unique index can prove
at-most-one cardinality when it applies to the same local column set as a
foreign key. Partial indexes, expression indexes, and operator classes are not
modeled in this first pass; they should not be used to prove relation
cardinality until the metadata can represent their predicates and expressions.

## Relation Cardinality

Relation result cardinality is derived from foreign-key and uniqueness metadata.

By default, relation selections are collection-valued. A relation may be treated
as at-most-one only when catalog constraints prove that cardinality.

For a single-column foreign key from `profiles.user_id` to `users.id`, if the
referencing table has a primary-key, unique constraint, or unique index on
`profiles.user_id`, then the reverse relation from `users` to `profiles` is
at-most-one.

```text
profiles.user_id -> users.id
```

In that case, a `users { profiles { ... } }` relation can produce at most one
profile for each user. The referenced direction, `profiles { users { ... } }`,
also points at one user row when the local foreign-key column is non-null. If the
foreign-key column is nullable, the referenced relation may be absent.

Resolved selections, planning, SQL generation, and result metadata consume this
catalog proof as one shared at-most-one result shape. Composite foreign keys use
the same proof rules described below.

## Composite Constraints

Composite primary keys, unique constraints, indexes, and foreign keys use the
same table-level metadata as single-column constraints.

```yaml
schema: public
name: tenant_profiles
object_type: table
columns:
  - name: tenant_id
    database_type: int4
    data_type: int
    not_null: true
  - name: user_id
    database_type: int4
    data_type: int
    not_null: true
  - name: display_name
    database_type: text
    data_type: text
    not_null: false

constraints:
  - name: tenant_profiles_pkey
    kind: primary_key
    columns: [tenant_id, user_id]

foreign_keys:
  - name: tenant_profiles_user_fkey
    columns: [tenant_id, user_id]
    references:
      schema: public
      table: users
      columns: [tenant_id, id]
```

For a composite foreign key, at-most-one cardinality is proven when the
referencing column set is covered by a primary-key or unique constraint on the
referencing table, or by a unique index that has the same semantics.

The proving column set must guarantee uniqueness for the foreign-key tuple. A
unique constraint on `[tenant_id, user_id]` proves at-most-one cardinality for a
foreign key using `[tenant_id, user_id]`. A wider unique constraint such as
`[tenant_id, user_id, locale]` does not prove at-most-one cardinality for the
foreign-key tuple because multiple rows may share the same `[tenant_id,
user_id]` with different `locale` values.

Composite metadata is also required for correct SQL join generation. The join
predicate must include every column pair in order:

```sql
tenant_profiles.tenant_id = users.tenant_id
AND tenant_profiles.user_id = users.id
```

## Validation Rules

Catalog loading should reject or report metadata that cannot be used
deterministically.

- A table object must have a schema and name.
- Column names must be unique within an object.
- Constraint column names must exist on the local table.
- Foreign-key local column names must exist on the local table.
- A foreign-key target must identify an existing schema and table.
- Foreign-key referenced column names must exist on the referenced table.
- Foreign-key local and referenced column lists must have the same length.
- Foreign-key selectors must be stable for generated paths.
- Ambiguous relation selections must produce diagnostics instead of choosing an
  arbitrary path.

## Open Gaps

- Define the authored overlay format and the complete generated-to-effective
  merge contract, including first-class relationships and visibility changes.
- Implement nominal native and table-backed enum metadata as specified by
  [Enumerated Types](enums.md).
- Represent partial and expression unique indexes, including the predicates and
  expressions needed to prove when they establish at-most-one cardinality.
- Represent `NULLS NOT DISTINCT` uniqueness so nullable keys can participate in
  singular-selection proofs when their exact null semantics are known.
