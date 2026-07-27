# Introspect native PostgreSQL type capabilities

**ID:** 4b1e4216 | **Status:** Done | **Created:** 2026-07-26T11:48:49+02:00

Type capabilities are asserted by a hardcoded table even though PostgreSQL can
state them. `type_map.yaml` already stores an `operations` set per type, and
nothing reads it. Meanwhile operator legality is decided by a match arm that
cannot know whether `citext` supports `like` or whether a range type supports
ordering.

Populate capability records from the provider. The index query already derives
per-key `native_operators` from `pg_amop` and `pg_operator`, so the pattern
exists in the introspection crate; the type query needs the equivalent over a
type's default operator families.

Column rows should also carry `atttypid` and `atttypmod` so a column points at a
type identity rather than a name, and `format_type(atttypid, atttypmod)` should
be retained for display so `varchar(20)` and `numeric(10,2)` survive to hover and
generated documentation.

Introspection must capture capabilities into the generated catalog. Compilation
stays offline and derives everything from committed YAML.

Sequenced after b50babe0.

Schema-qualified [`TypeKey`] plumbing for type and column identity landed with
issue 17c6cc60. This issue still owns provider OIDs/modifiers, formatted display
names, type kind/category, and capability derivation. Formatted type names must
also be made independent of the introspection connection's `search_path`.

Acceptance criteria:

- `type_map.yaml` records type kind, category, and a derived capability set per
  type.
- Capability records are built from provider facts, with the hardcoded table
  retained only as the fallback for fixtures and tests that have no provider.
- Column metadata references a type identity and retains its formatted display
  name including type modifiers.
- A capability change in the database is visible after `dsql introspect` without
  a compiler change.
- Introspection tests cover a type whose operator set differs from the hardcoded
  assumption.

## Resolution

PostgreSQL introspection now reads columns and type capabilities in one
repeatable-read snapshot. Transaction-local OIDs join each column to its
schema-qualified [`TypeKey`]; committed metadata retains the exact formatted
type, modifier, raw type kind/category, native operations, and ordering support
without persisting OIDs.

Fresh provider facts replace the compiler's comparison and ordering defaults.
The builtin matrix remains the compatibility fallback for metadata without the
optional provider record. DSQL still owns provider-independent behavior such as
literal categories, defaults, and aggregate result typing.

Existing committed catalog fixtures deliberately retain their older
provider-less type maps so they continue to test that fallback. Regenerating
those snapshots from a live catalog is tracked separately.
