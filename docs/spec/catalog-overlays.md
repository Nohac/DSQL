# Catalog Overlays

Status: implemented for version 1; deferred extensions remain design work.

Catalog overlays are authored, reviewable modifications to a provider-generated
catalog. They add application knowledge that introspection cannot prove, change
which provider facts are exposed to queries, and retain enough provenance to
distinguish database shape from project assertions.

Overlays never modify the generated catalog in place. Project loading produces
one validated effective catalog, and every compiler consumer reads the same
`CatalogSnapshot` built from it.

## Ownership Boundary

Catalog construction has four distinct stages:

```text
database or metadata provider
        -> generated catalog under dsql/schema/
        -> authored overlays under dsql/overlays/
        -> validated effective catalog
        -> CatalogSnapshot
```

- The provider owns the generated catalog. PostgreSQL introspection may replace
  it completely.
- The project owns overlay documents. Introspection never writes them.
- Project loading owns composition, validation, and provenance.
- Resolution, checks, policies, planning, SQL, editor services, and generation
  consume only the effective `CatalogSnapshot`. They do not merge sources or
  reinterpret overlay assertions independently.

Deleting every overlay must recover the provider-generated catalog's semantics.
Deleting and regenerating `dsql/schema/` must not remove authored intent.

## Files And Discovery

Overlay documents are YAML files discovered recursively under:

```text
dsql/overlays/**/*.yaml
```

The overlay directory is fixed at project-base-relative `dsql/overlays/`, a
sibling of `dsql/dsql.toml`. The daemon returns this path as `overlaysDir`
during initialization so consumers do not duplicate the convention.

Every file has an explicit format version:

```yaml
version: 1
objects: []
```

Paths are sorted lexically for deterministic diagnostics and serialization, but
path order has no semantic precedence. An overlay cannot override another file
merely because it sorts later.

Object references are always structurally schema-qualified so database
identifiers are not parsed from a delimiter-containing string:

```yaml
target:
  schema: public
  name: order_summary
```

All column and relationship names are exact, case-sensitive catalog names.

## Version 1 Shape

One object patch may override documentation, assert view uniqueness, add
directional relationships, or change visibility:

```yaml
version: 1

objects:
  - target:
      schema: public
      name: order_summary

    description: Read model used by the operations dashboard.

    columns:
      - name: customer_id
        description: Stable customer reference copied into the read model.
      - name: internal_payload
        hidden: true

    assert_unique:
      - name: order_summary_id_unique
        columns: [id]

    relationships:
      - name: customer
        target:
          schema: public
          name: customers
        columns:
          - local: customer_id
            target: id

      - name: line_items
        target:
          schema: public
          name: order_items
        columns:
          - local: id
            target: order_id

    hide:
      relationships:
        - target:
            schema: public
            name: legacy_customers
          selector: legacy_customer_id
          direction: referencing
```

An entire provider object may be hidden:

```yaml
version: 1
objects:
  - target:
      schema: audit
      name: raw_events
    hidden: true
```

Object and column `description` fields accept a string and replace the
corresponding effective description. Version 1 cannot clear a provider
description without replacing it with an authored string.

Each `columns` entry targets one provider column by exact name. `description`
overrides its documentation, while `hidden: true` removes it from the exposed
catalog. Version 1 has no `hidden: false`: absence of a hide is not an override
of another overlay's ownership.

Each `assert_unique` entry is a trusted assertion that the ordered, non-empty
column set is unique. It is allowed only when the generated target has
`object_type: view` or `object_type: materialized_view`, where PostgreSQL may
not expose a usable constraint. Its `name` is an authored stable identity used
by conflicts, diagnostics, provenance, and future catalog inspection.

`assert_unique` does not create a database constraint or index and does not
override provider metadata. It supplies effective-catalog evidence used to
infer relationship cardinality and generated result types. Overlay documents
have no facility for adding, hiding, weakening, or replacing provider
constraint, index, or uniqueness facts.
Hiding a relationship derived from a foreign key changes only query-facing
exposure; the underlying constraint fact remains available as proof support.

**Safety boundary:** the database does not enforce an overlay assertion and
catalog validation does not scan live rows. If the asserted columns contain
duplicates, DSQL may generate a singular result contract for data that is
actually plural. That can cause a database error at execution time or violate
the generated result type's cardinality. The project author is responsible for
maintaining the invariant, including across materialized-view refreshes.
Ordinary tables must express uniqueness through database DDL and
re-introspection instead.

Each relationship is exposed only from the object containing its declaration.
`columns` is an ordered, non-empty list mapping `local` columns on that object
to `target` columns on the relationship target. No inverse relationship is
synthesized. A project that needs both directions declares both directions.

`hide.relationships` addresses provider-derived relationships by
schema-qualified target, generated selector, and direction. The selector is
always formed from the foreign key's ordered columns on its referencing side;
composite selectors join those columns with `_`, such as
`tenant_id_user_id`.

- `direction: referencing` means the patched object owns the foreign-key
  columns and the target is the referenced object. The example above hides
  `public::legacy_customers->legacy_customer_id`.
- `direction: referenced` means the patched object is the referenced object
  and the target owns the foreign-key columns. For example, a patch on
  `public::customers` with target `public::orders`, selector `customer_id`,
  and `direction: referenced` hides the reverse edge from customers to orders.

Direction is required even when the other fields happen to identify one edge.
It distinguishes both exposed directions of a provider foreign key and remains
unambiguous for self-referential foreign keys. Hiding one direction never hides
the other. The canonical hide key is the patched object, target, selector, and
direction. Missing or ambiguous hide targets are errors.

An overlay relationship is authored input; removing its declaration is how it
is removed. Version 1 relationship hides apply to provider-derived edges, not
to relationships declared by another overlay.

## Composition And Conflicts

Multiple files may patch the same object when their operations touch different
semantic keys. Composition is set-based and independent of file order.

The following are project-load errors:

- more than one overlay writes an object's `description` or `hidden` state;
- more than one overlay writes the same column's `description` or `hidden`
  state;
- an object patch combines `hidden: true` with any other property, or another
  overlay patch targets that hidden object;
- a column patch combines `hidden: true` with `description`, or another
  overlay patch targets that hidden column;
- the same provider relationship is hidden more than once;
- an `assert_unique` entry targets a table or any object kind other than
  `view` or `materialized_view`;
- uniqueness assertion names are duplicated within one object;
- two uniqueness assertions cover the same ordered column tuple;
- an overlay uniqueness assertion name collides with a provider constraint or
  index name on the object;
- an overlay uniqueness assertion repeats the ordered column tuple of a
  provider uniqueness proof;
- overlay relationship names are duplicated within one object;
- an overlay relationship name collides with a visible column or any visible
  provider-derived relationship name after provider hides are applied; or
- two authored operations otherwise claim the same semantic key, even when
  their values happen to be identical.

There is no last-writer-wins rule, file priority, implicit shadowing, or silent
deduplication. Redundant declarations are errors because they create ambiguous
ownership during later edits.

If a materialized view later gains a provider unique index covering an asserted
tuple, re-introspection makes the overlay assertion redundant and therefore
invalid. The repair is to remove `assert_unique` and use the provider proof.

Hidden columns may still be named by `assert_unique` assertions and relationship
join mappings. Those references are internal proof dependencies, not patches
to the hidden column and not query-facing exposure.

Provider relationship hides are resolved first. Overlay relationships are then
added and checked against the remaining exposed field namespace. The supported
way to replace a provider-derived exposure with an authored name is therefore
explicit: hide the provider edge and add the named overlay relationship. The
new relationship may reuse the same physical mapping and provider proof, but
its exposed name and declaration provenance remain authored.

Existing provider-provider ambiguity remains selectable through explicit edge
selectors as described by [Catalog Metadata](catalog-metadata.md). Overlay
relationship names are intentionally stricter: an authored name must be an
unambiguous ordinary field selection.

## Provenance And Proofs

Provenance is not one `generated` or `overlay` flag. An effective catalog fact
can be declared by one source while separate sources support its join,
cardinality, presence, documentation, or visibility.

Every support record contains:

- `kind`: provider or overlay;
- the source file;
- a stable semantic item path within that file; and
- a byte range when the source decoder can provide one.

Generated provider facts point into `dsql/schema/`. Authored facts point into
`dsql/overlays/`. A metadata provider without files supplies an equivalent
stable provider identity and semantic item path; it is not misreported as an
overlay.

Effective relationship provenance keeps distinct support classes:

- **declaration support**: why the exposed relationship exists and what owns
  its name;
- **join support**: the ordered column mapping and whether it matches a
  provider foreign key;
- **cardinality support**: the provider constraint/index or overlay
  `assert_unique` assertion proving the target tuple unique;
- **presence support**: the provider foreign key and source-column nullability
  facts, when they prove a related row must exist; and
- **exposure support**: overlay facts that hide catalog fields or relationships.

For example, an overlay may name `customer`, its mapping may exactly match a
PostgreSQL foreign key, and target uniqueness may come from a PostgreSQL primary
key. The declaration is authored while its join, cardinality, and possible
presence proofs remain provider-backed. Conversely, a view-to-view relationship
and its cardinality proof may both be overlay assertions.

Derived facts retain all supports used to reach the result. Consumers use the
effective semantics rather than branching on provenance, but catalog
diagnostics, editor navigation, debug tooling, and generator-facing catalog
metadata can inspect the supports. Provenance must never be reconstructed from
names after composition.

## Relationship Validation And Semantics

An overlay relationship is valid only when:

- the declaring and target objects exist in the generated catalog;
- every local and target column exists;
- the mapping is non-empty and maps the same number of columns on each side;
- neither side repeats a column;
- each mapped pair has exactly the same nominal provider type identity and
  exactly the same logical type;
- the target remains exposed; and
- its authored name is valid and collision-free in the declaring object's
  effective field namespace.

The nominal provider identity is `(schema, internal_type)`, resolved through
the generated `type_map.yaml` entry for each column's `database_type`. This
uses the existing generated type metadata; it does not require table YAML to
repeat the type schema. An absent or ambiguous type-map entry makes an overlay
join invalid.

Version 1 does not insert casts or transformation expressions into manual
joins. Cross-width numeric types, domains and their base types, types connected
only by a provider equality operator, and values that are merely convertible
are not compatible. More permissive provider-aware compatibility is deferred.

### Provider-backed mappings

Composition compares each authored mapping with provider foreign keys in both
directions. An exact ordered match retains that foreign key as join support.
The relationship remains overlay-declared; matching provider support does not
change ownership of its name.

No match is required. An overlay-only mapping is a trusted assertion that the
columns may be joined, but it does not assert that either side contains a
matching row.

### Cardinality

Cardinality is inferred, never declared by the overlay relationship.

The relationship is singular when the effective catalog proves that its target
column tuple is unique. Proof may come from a provider primary key, unique
constraint, supported unique index, or an allowed overlay `assert_unique`
assertion on a view or materialized view. The same tuple rules used for
provider-derived relationships apply: a wider unique set does not prove a
narrower mapping unique, and unsupported partial or expression indexes do not
participate.

Without a target uniqueness proof, the relationship is collection-valued.
Collections always produce arrays.

### Singular nullability

Uniqueness proves at-most-one; it does not prove existence.

A singular relationship is non-null only when all of the following are true:

1. its mapping exactly follows a provider-recognized foreign key from the
   declaring object to the target;
2. that provider foreign key supplies the same existence guarantee used for an
   ordinary provider-derived relation; and
3. every local foreign-key column is non-null.

Every other singular overlay relationship is nullable. In particular:

- an overlay `assert_unique` assertion does not prove a matching row exists;
- a reverse provider relationship remains nullable even when the referencing
  tuple is unique;
- a view relationship without an enforced provider foreign key is nullable;
  and
- non-null local columns alone do not prove referential existence.

These rules make an overlay relationship behave exactly like a physical
relationship when it has the same provider proofs, while failing safely when it
does not.

## Visibility

Hiding changes the exposed catalog; it does not delete physical facts.

A hidden object cannot be selected as a root or exposed through a visible
relationship. Its fields do not participate in completion, hover, structural
filter matching, query predicates, or generated public metadata.

A hidden column is unavailable to query source in every position, including
selection, predicates, ordering, grouping, aggregates, and policy shape
matching. It remains available internally as a join operand, constraint member,
or proof dependency. Hiding a join column does not implicitly hide an otherwise
valid relationship.

A hidden provider relationship is removed from field resolution and editor
services but its foreign-key fact remains available as support for an authored
relationship with the same mapping.

Provider-derived relationships pointing to a hidden object are hidden with the
object. Declaring a visible overlay relationship to a hidden target is an error,
not an implicit re-exposure.

Explicitly hiding a provider relationship whose target is already hidden is a
redundant patch and therefore an error.

Descriptions and visibility carry overlay provenance. Go-to-definition from an
authored relationship or overridden description should prefer the overlay
declaration; inspection can still show the provider facts supporting it.

## Effective Catalog Construction

Project loading performs one deterministic transaction in memory:

1. Load and structurally validate the provider-generated catalog.
2. Discover, parse, and version-check every overlay document.
3. Resolve every overlay object, column, uniqueness assertion, and provider-edge
   target against generated facts. Missing targets are not ignored.
4. Detect cross-file ownership conflicts before applying any patch.
5. Apply object/column descriptions, allowed view-uniqueness assertions,
   visibility, and provider-edge hides while retaining their provenance.
6. Construct visible provider relationships, then add overlay relationships and
   attach matching provider join support.
7. Derive cardinality and nullability from the complete effective proof set.
8. Validate the exposed object and field graph, including names and hidden
   targets.
9. Construct and fingerprint one effective `CatalogSnapshot`.

Any failure aborts the entire effective catalog. No consumer observes a partial
overlay application or a mixture of previous and current catalog facts.

Stable generated identities are semantic schema/object/column/constraint keys,
not vector indexes or source line numbers. Internal IDs may be rebuilt after
composition as long as every reference in the snapshot is self-consistent.

## Validation Commands

The catalog-only validation command is:

```text
dsql catalog validate
```

It is read-only and does not connect to the database. It parses project
configuration, loads the generated catalog and every overlay, constructs the
effective catalog, and reports catalog diagnostics. It validates an
`assert_unique` declaration structurally but cannot verify it against live view
rows. It does not discover or compile DSQL documents, update `dsql.lock`,
publish build artifacts, or run host generators.

The existing `dsql validate` command is a superset: successful full validation
implies successful catalog validation before document analysis begins. Every
other project entry point uses the same catalog loader and cannot bypass overlay
validation.

### Introspection

`dsql introspect` first validates the candidate provider metadata structurally.
On success it transactionally replaces the generated catalog, because generated
facts must continue to reflect the database even when authored overlays have
become stale. It then composes and validates the overlays against that new
catalog. Overlay failure cannot leave a partially written generated snapshot.

If an underlying table or column was renamed, introspection exits non-zero with
the overlay diagnostics after the generated catalog has been updated. It does
not roll back to a misleading older snapshot. The user can review the generated
change, repair the overlay, and run `dsql catalog validate` without reconnecting
to the database.

`dsql introspect --dry-run` performs the same structural and overlay validation
against the in-memory candidate but writes neither generated nor authored
files.

## Diagnostics

Catalog failures should identify authored intent and the provider evidence it
failed against. Diagnostics are deterministic and include, where applicable:

- overlay file and semantic YAML item path;
- the missing schema, object, column, constraint, or provider edge;
- schema-qualified near candidates from the generated catalog;
- the generated catalog location for an incompatible target;
- both authored locations in an ownership conflict;
- the mapped column pair and incompatible types;
- the field that collides in the effective namespace; and
- a concise repair hint, such as updating the target, removing a redundant
  patch, or rerunning introspection when generated metadata is absent.

Representative failures include:

```text
dsql/overlays/read-models.yaml objects[0].relationships[0].columns[0]:
column public.order_summary.customer_id was not found
candidate: public.order_summary.client_id
```

```text
dsql/overlays/read-models.yaml objects[0].assert_unique[0]:
uniqueness assertion order_summary_id_unique references missing column id
generated object: dsql/schema/public/order_summary.yaml
```

Overlay parse and effective-catalog validation errors are project-load failures,
not language diagnostics attached to a query document.

## Daemon And Editor Lifecycle

The daemon never watches the filesystem. Consumers continue to watch the
project and forward changes through `filesChanged`.

Any changed path under `dsql/overlays/` is a project-input change, just like
`configPath` or `schemaDir`, and triggers transparent full reload. Overlay paths
must not be added to watch exclusions; they are inputs, not generated outputs.

If reload fails, the attempted resident bowl is discarded and the request
answers the existing `ProjectLoadFailed` error with an actionable overlay path
and message. The last published build tree remains untouched. Subsequent
`compile` or `filesChanged` requests retry full project loading until the
catalog becomes valid, following the [build daemon](build-daemon.md) contract.

Editor services operate on the last successfully loaded project only. Catalog
navigation uses provenance to choose generated YAML for provider facts and
overlay YAML for authored relationships, visibility, documentation, and
view-uniqueness assertions.

## Non-Goals For Version 1

Version 1 does not support:

- virtual or computed columns;
- changing a provider column's physical type, logical type, or nullability;
- renaming physical schemas, objects, or columns;
- adding, hiding, weakening, replacing, or otherwise changing provider
  constraint, index, or uniqueness facts (relationship-edge hiding changes
  exposure only);
- asserting uniqueness for ordinary tables or object kinds other than views
  and materialized views;
- arbitrary SQL, predicates, casts, or transformation expressions;
- synthesized inverse relationships;
- global naming rewrite rules;
- priorities or last-writer-wins composition;
- using an overlay to mutate the generated schema files; or
- treating `assert_unique` as proof of referential existence.

Explicit hide-plus-add is the supported relationship naming mechanism. It
changes the exposed relationship declaration while preserving any independently
matching provider proofs; it does not rename the underlying database objects or
constraints.

## Implementation Sequence

1. Add versioned overlay decoding, source provenance, and catalog-only
   validation without changing effective semantics.
2. Split provider metadata from the effective catalog builder and preserve
   object kinds through `CatalogSnapshot`.
3. Add description overrides and visibility.
4. Add view-only `assert_unique` assertions and derived proof provenance.
5. Add directional named relationships, provider mapping matches, cardinality,
   and nullability.
6. Integrate project reload, CLI diagnostics, editor navigation, and metadata
   consumers, then enable the feature as one coherent catalog boundary.

Each step must keep the compiler consuming one effective snapshot. Temporary
paths where some systems read generated metadata and others read overlays are
not valid intermediate architecture.

## Deferred Extensions

- Exact byte ranges for YAML items when the selected decoder cannot yet expose
  spans. File paths and stable semantic item paths are required in version 1.
- A read-only `dsql catalog show` or `dsql catalog explain` command for
  inspecting effective facts and their proof provenance.
- Explicitly clearing provider descriptions.
- Provider-independent relationship selector syntax beyond the current DSQL
  edge identity.
- FK-less enum attachments and other closed-set assertions described by
  [Enumerated Types](enums.md).
