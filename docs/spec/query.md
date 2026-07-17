# Query Language

Status: in progress.

dsql is a domain specific query language for describing read query shape over a
relational catalog. The language is nested like GraphQL where it describes
result shape, and SQL-like where it filters, orders, and slices data.

## Example

```dsql
fragment PostSummary on public::posts {
  id
  title
}

query UsersWithPosts {
  public::users(where .id > 100 limit 20) {
    id
    display_name: name
    posts(order by created_at desc limit 5) {
      ...PostSummary
    }
  }
}
```

This describes a result rooted at `users`. `id` and `name` resolve to columns on
`public::users`. `posts` resolves through catalog relationship metadata from
`public::users` to `public::posts`. The fragment is valid because it targets the
same table as the current relation selection.

## Documents

A document is a sequence of top-level declarations.

```dsql
query Users {
  users {
    id
  }
}

fragment UserName on users {
  name
}
```

Core declarations:

- `query`
- `fragment`

## Names

Ordinary identifiers use this shape:

```text
[A-Za-z_][A-Za-z0-9_]*
```

Schema-qualified table references use `::` between the schema segment and the
table or view segment:

```dsql
public::users
other_schema::users
```

Relationship references may add a relation edge selector after the table
reference:

```dsql
public::users->assignee_id
users->reviewer_id
```

The part before `->` is still the table reference. The part after `->` selects
which foreign-key path connects the current table to that relation target. The
arrow is intentionally pointer-like: it points at the specific relation edge to
use when multiple edges reach the same table.

All catalog schemas in the selected resolution environment are visible for table
resolution. An unqualified table reference is valid only when exactly one
visible schema contains a table or view with that name.

For example, `users` resolves as `public::users` if `public` is the only visible
schema with a `users` object. If both `public::users` and `auth::users` exist,
the unqualified reference is ambiguous and must be written with a schema
selector.

The output key for a schema-qualified table or relation is still the table name
unless an alias is provided. For example, `public::users` produces `users`.

The output key for a relation with a relation edge selector is also the table
name unless an alias is provided. For example, `users->assignee_id` produces
`users`.

## Queries

A query declaration has a required query name and a root selection set.

```dsql
query Users {
  users {
    id
    name
  }
}
```

Anonymous queries are not part of the language.

## Selection Sets

A selection set is a brace-delimited list of selections.

```dsql
{
  id
  name
  posts {
    id
  }
}
```

Commas may be used as optional separators.

```dsql
query Users {
  users {
    id, name, email
    posts {
      id,
      title,
    }
  }
}
```

Formatting should preserve the user’s line grouping when commas are present.
For example, `id, name, email` should remain on one line. Selection lists without
commas may be formatted as one selection per line.

The default formatter line width is 100 characters. Clause lists should stay
inline when they fit within that width. If a clause list does not fit, the
formatter may break at clause boundaries while preserving short user line groups
where possible.

## Field Selections

A field selection names a column, relation, or future catalog-backed field in
the current object context.

```dsql
id
name
posts {
  id
}
```

Scalar fields resolve to columns and do not have subselections.

Relation fields resolve through catalog relationship metadata and have
subselections.

```dsql
query Users {
  users {
    id
    posts {
      id
      title
    }
  }
}
```

## Flattened Relation Selections

A leading `...` merges an object-valued relation selection into its parent
instead of emitting a wrapper object:

```dsql
query Users {
  users {
    id

    ...profile {
      display_name: name
      avatar_url
    }
  }
}
```

Conceptual result shape:

```json
{
  "users": [
    {
      "id": 1,
      "display_name": "Ada",
      "avatar_url": "https://example.test/ada.png"
    }
  ]
}
```

Flattening is valid exactly when the selection result is object-valued. A root
or relation selection made singular by catalog metadata, its predicates, or a
literal `limit 1` may flatten; a collection-valued selection may not. A
collection transform that produces one object, such as an ungrouped aggregate,
may also flatten. Array-valued transforms, including grouped aggregates, may
not. See [Selection Result Cardinality](#selection-result-cardinality).

If the singular selection may be absent because no row matches, a foreign key
is nullable, a clause suppresses the row, or an applicable policy filters it
out, every field contributed through the flatten inherits that absence and is
nullable in the result contract. Otherwise each contributed field keeps its
ordinary column or nested-result nullability.

Flattened fields participate in the same output-key length, duplicate, and
fragment-expansion collision checks as direct fields. The compiler never
silently overwrites or merges colliding flattened output.

Schema qualification, relation-edge selectors, and relation clauses remain
available:

```dsql
query Tasks {
  tasks {
    id
    ...public::users->assignee_id {
      assignee_name: name
    }
  }
}
```

An alias cannot wrap a flattened relation because there is no wrapper output
key. Aliased fragment spreads remain a separate proposed form.

Ellipsis syntax is classified by what follows the name:

```text
...Name                         fragment spread
...relation { ... }             flattened relation selection
...relation | aggregate { ... } flattened object-producing transform
```

This also leaves future fragment binding lists unambiguous: binding items begin
with `$` or `$$`, while relation clauses begin with clause keywords. Empty
ambiguous parentheses are diagnostics. Once an ellipsis selection is classified
as a relation, omitting both its selection set and pipe transform is invalid.

A query root without an at-most-one proof is collection-valued and cannot
flatten. A singular root selection or ungrouped root aggregate is object-valued
and may merge its fields into the query result object. See
[Aggregates](aggregates.md#flattened-output).

## Dotted Selection Paths

Status: proposed extension.

Dotted selection paths are shorthand for selecting through a nested relation or
object path.

```dsql
query Movies {
  movies {
    id
    title
    director.name
    director.profile.image_url
  }
}
```

Unaliased dotted selections preserve the nested result shape. The example above
is equivalent to:

```dsql
query Movies {
  movies {
    id
    title
    director {
      name
      profile {
        image_url
      }
    }
  }
}
```

A dotted path is resolved from left to right in the current selection context.
Intermediate segments must resolve to relation or object-valued fields. The
final segment may resolve to a scalar field or to a relation when the selection
has a subselection.

Dotted scalar selections do not flatten output by default. Flattening requires
an explicit alias:

```dsql
query Movies {
  movies {
    director_name: director.name
  }
}
```

The alias projects the terminal scalar value into the current object under the
alias key. Flattening through a collection-valued relation is invalid unless a
future aggregate or list-projection feature defines the behavior explicitly.

## Relationship Chain Selections

Status: proposed extension.

Relationship chain selections allow a relation path to be written as one
selection while still attaching clauses to individual path segments.

```dsql
query Movies {
  movies {
    cast(where .role == "lead" limit 5).actor {
      name
      profile.image_url
    }
  }
}
```

This is equivalent to:

```dsql
query Movies {
  movies {
    cast(where .role == "lead" limit 5) {
      actor {
        name
        profile {
          image_url
        }
      }
    }
  }
}
```

Clauses belong to the segment they follow.

```dsql
cast(where .role == "lead" limit 10).actor {
  name
}
```

The `where` and `limit` clauses apply to `cast`.

```dsql
cast.actor(where .name like $$actor_name) {
  name
}
```

The `where` clause applies to `actor`.

Conceptual grammar:

```text
selection_path = relation_step ("." relation_step)*
relation_step  = relation_ref clause_list?
selection      = alias? selection_path directives? selection_set?
```

Unaliased relationship chains preserve nested shape:

```dsql
cast.actor {
  name
}
```

Conceptual result shape:

```json
{
  "cast": [
    {
      "actor": {
        "name": "Greta Lee"
      }
    }
  ]
}
```

An alias on a relationship chain projects the terminal relation under the alias:

```dsql
performers: cast.actor {
  name
}
```

Conceptual result shape:

```json
{
  "performers": [
    {
      "name": "Greta Lee"
    }
  ]
}
```

This is not only formatter sugar. Alias projection affects result shape,
cardinality, generated types, variables, fragments, and SQL generation, so it
should be implemented as an explicit language feature.

Dotted selection paths and relationship chains share the same semantic rule:
each segment is checked against the result of the previous segment. Since schema
qualification uses `::`, `.` is available for selection traversal without
overlapping with schema-qualified table references.

## Aliases

A selection may be aliased.

```dsql
query Users {
  users {
    user_id: id
    display_name: name
  }
}
```

The alias affects only the output key. Field resolution still uses the original
field name.

Output keys are runtime result keys. They must be valid PostgreSQL result
aliases and must be at most 63 bytes. This limit applies to explicit aliases and
to inferred output keys.

Aliases are used when two selections would produce the same output key.

```dsql
query Users {
  public_users: public::users {
    id
  }

  other_users: other_schema::users {
    id
  }
}
```

Without aliases, both root selections would output `users`.

## Catalog Resolution

The catalog is the source of truth for tables, columns, primary keys, indexes,
and foreign-key relationships.

Root selections resolve against catalog tables or views.

```dsql
query Users {
  users {
    id
  }
}
```

Unqualified roots are valid only when the table or view name is unique across
visible schemas. Ambiguous names are diagnostics:

```text
ambiguous table reference `users`
candidates:
  public::users
  auth::users
use `schema::users` to disambiguate
```

Schema-qualified roots are explicit and bypass ambiguity:

```dsql
query Users {
  other_schema::users {
    id
  }
}
```

Relationship names are catalog-driven. The language does not singularize,
pluralize, or otherwise rewrite them by itself.

Nested relation selections resolve from the current table's catalog
relationships. They do not scan every visible schema the way root table
references do. If the relation target name is ambiguous from the current table,
use `schema::table` and, when needed, `->edge` to identify the intended
relationship.

Catalog metadata may prove that a relationship itself is at-most-one, for
example through a unique or primary-key constraint over the foreign-key column
set. This is one of the proofs that determines a selection's result shape. See
[Selection Result Cardinality](#selection-result-cardinality) and
[Catalog Metadata](catalog-metadata.md#relation-cardinality).

Qualified relation references are allowed:

```dsql
query Users {
  public::users {
    public::posts {
      id
    }
  }
}
```

The output key for `public::posts` is still `posts`.

If multiple foreign-key paths connect the current table to the same relation
target, the relation must include a relation edge selector.

```dsql
query Tasks {
  tasks {
    users->assignee_id {
      id
    }
  }
}
```

The selector identifies the foreign-key path. The default selector is derived
from the foreign-key column names for that path. For a single column foreign
key, the selector is the local column name, such as `assignee_id`. For a
composite foreign key, the selector joins the local column names in constraint
order with underscores, such as `tenant_id_order_id`.

Schema qualification and relation edge selectors may be combined:

```dsql
query Tasks {
  tasks {
    public::users->reviewer_id {
      id
    }
  }
}
```

When only one relation edge connects the current table to the relation
target, the selector may be omitted.

## Selection Result Cardinality

Every root and relation selection has a statically determined result
cardinality. A collection-valued selection produces an array. An at-most-one
selection produces an object or `null`; it never produces a one-element array.
At-most-one controls the container shape. The object's nullability still
reflects whether the selected row may be absent because no row matches, a
relationship is optional, a clause suppresses it, or an applicable policy
filters it out.

A selection is at-most-one when any of these independent proofs applies:

1. The selected relationship is catalog-proven at-most-one as described by
   [Catalog Metadata](catalog-metadata.md#relation-cardinality).
2. Predicates that are present in every compiled variant constrain every column
   of one catalog-known primary key, unique constraint, or supported unique
   index with equality to fixed values or variables. Each proving predicate
   must be mandatory, and the proof must remain outside alternatives that could
   bypass it. Additional predicates may narrow the result further.
3. The selection has the compile-time integer literal `limit 1`.

Otherwise the selection is collection-valued. In particular, `limit $$count`,
an optional limit, and other runtime limit expressions do not themselves prove
at-most-one cardinality. A separate relationship or unique-predicate proof
still applies when such a limit is present.

Unique-predicate proofs are conservative:

- A composite key is covered only when every key column has a proving equality.
- `or` branches do not prove uniqueness unless the same complete proof is
  mandatory for every branch.
- A conditionally omitted predicate does not participate. A caller-omittable
  variable with a default may participate when the predicate remains present
  in every compiled variant. See [Variables](variables.md).
- Equality to another column or another row-dependent expression does not
  prove uniqueness.
- For nullable unique-key columns, the compiled equality must not match SQL
  `NULL`. A null-matching predicate cannot use ordinary PostgreSQL uniqueness
  semantics as proof because multiple null key values may exist. Catalog
  metadata does not yet model `NULLS NOT DISTINCT` constraints.
- Partial and expression unique indexes do not participate until catalog
  metadata can represent and prove their predicates and expressions.

`limit 1` determines the result shape whether or not the selection has an
`order by` clause. Without stable ordering, which matching row is selected is
unspecified. An `offset`, a zero-valued runtime limit, or a policy may turn a
proven singular result into `null`, but cannot broaden it back into a
collection.

Inferred cardinality is part of the semantic result shape. Planning, SQL
generation, result metadata, generated API types, flattening, fragment merging,
and editor services must consume the same resolved cardinality rather than
re-deriving it independently.

## Relationship Ambiguity

A relation selection is ambiguous when it names a relation target but multiple
foreign-key paths connect the current table to that target.

For example, ambiguity exists if `public::tasks` has both of these foreign keys
to `public::users`:

```text
public.tasks.assignee_id references public.users.id
public.tasks.reviewer_id references public.users.id
```

Then this selection has no way to choose which foreign key to use:

```dsql
query Tasks {
  tasks {
    users {
      id
    }
  }
}
```

The selection must disambiguate the relation edge:

```dsql
query Tasks {
  tasks {
    users->assignee_id {
      id
    }
  }
}
```

If multiple selected relations would produce the same output key, aliases are
required.

```dsql
query Tasks {
  tasks {
    assignee: users->assignee_id {
      id
    }

    reviewer: users->reviewer_id {
      id
    }
  }
}
```

Without aliases, both selections would output `users`.

## Clauses

Selection clauses are written inside parentheses after a root or relation
selection.

```dsql
query Posts {
  posts(where .created_at >= "2026-01-01" order by created_at desc limit 20) {
    id
    title
  }
}
```

Core clauses:

- `where <predicate>`
- `order by <field> <direction>[, ...]`
- `limit <integer>`
- `offset <integer>`

### Where

`where` filters the current root or relation selection.

```dsql
query ActiveUsers {
  users(where .active == true) {
    id
    name
  }
}
```

Predicates resolve field names against the selected table or relation target.

Scoped predicate paths for relationship filters and parent/root references are
tracked separately in [Scoped Predicates](scoped-predicates.md).

Core predicate operators:

```text
==
!=
>
>=
<
<=
```

Predicates may be combined with boolean `and` and `or`.

```dsql
query ActiveRecentUsers {
  users(where .active == true and .created_at >= "2026-01-01") {
    id
    name
  }
}
```

`and` and `or` are infix operators between predicates. `and` binds tighter than
`or`, and parentheses may be used to group boolean expressions explicitly.

```dsql
query Users {
  users(where (.active == true or .trial == true) and .deleted_at == null) {
    id
  }
}
```

The predicate grammar should follow this shape:

```text
predicate_expr = or_expr
or_expr        = and_expr ("or" and_expr)*
and_expr       = primary_expr ("and" primary_expr)*
primary_expr   = comparison | "(" predicate_expr ")"
comparison     = field_path operator value_or_field_path
```

Each side of a boolean operator must be a predicate expression. Bare field paths
are not valid boolean predicates by themselves.

### Order By

`order by` sorts the current collection.

```dsql
query RecentPosts {
  posts(order by created_at desc, id asc limit 20) {
    id
    title
  }
}
```

Sort direction is `asc` or `desc`.

### Limit And Offset

`limit` and `offset` slice the rows produced by a selection.

```dsql
query Page {
  posts(order by created_at desc limit 20 offset 40) {
    id
    title
  }
}
```

`limit` and `offset` values are non-negative and integer-compatible. Their
effect on the result container is defined by
[Selection Result Cardinality](#selection-result-cardinality).

Limits on a selection that is independently catalog-proven at-most-one receive
these diagnostics:

- A positive integer literal is redundant because it cannot reduce the maximum
  cardinality.
- A runtime limit cannot further bound the cardinality. Within the valid
  non-negative domain, it affects only whether the possible row is suppressed
  when the value is zero.

Literal `limit 0` on any selection is diagnosed as always empty rather than as
a redundant limit.

Literal `limit 1` on a selection without another at-most-one proof is not
redundant: it changes the result from an array to a nullable object. A positive
literal greater than one does not change collection cardinality. `offset` may
suppress rows but never proves or broadens cardinality.

## Values

Core literal values:

```dsql
123
12.5
true
false
null
"text"
```

## Fragments

Fragments define reusable selection sets for a catalog target.

```dsql
fragment UserSummary on public::users {
  id
  name
}

query Users {
  users {
    ...UserSummary
  }
}
```

The fragment target resolves through the same table-resolution rules as root
selections. A fragment spread is valid only when the fragment target matches the
current selection context.

```dsql
fragment PostSummary on posts {
  id
  title
}

query Users {
  users {
    posts {
      ...PostSummary
    }
  }
}
```

Fragments may be defined before or after the query that spreads them.

### Fragment Spread Aliases And Merging

Status: proposed extension.

Conceptual syntax:

```text
fragment_spread = alias? "..." fragment_name binding_list? directives?
```

`binding_list` uses the bound definition input syntax described in the
variables spec.

An unaliased fragment spread merges the fragment selection set into the current
selection object.

```dsql
fragment UserSummary on users {
  id
  name
}

query Users {
  users {
    ...UserSummary
  }
}
```

An aliased fragment spread wraps the fragment selection set under the alias in
the result shape:

```dsql
fragment RecentPosts on users {
  posts(where .created_at > $recent_after) {
    id
    title
  }
}

fragment PopularPosts on users {
  posts(where .score > $min_score) {
    id
    title
  }
}

query Users {
  users {
    recent: ...RecentPosts($recent_after <- $after)
    popular: ...PopularPosts($min_score <- $score)
  }
}
```

Conceptual result shape:

```json
{
  "users": [
    {
      "recent": {
        "posts": [{ "id": "...", "title": "..." }]
      },
      "popular": {
        "posts": [{ "id": "...", "title": "..." }]
      }
    }
  ]
}
```

The alias applies to the spread as a wrapper object. It does not rename each
field inside the fragment. If a flatter shape is desired, the fields inside the
fragment should be aliased instead:

```dsql
fragment RecentPosts on users {
  recent_posts: posts(where .created_at > $recent_after) {
    id
    title
  }
}
```

Duplicate selections contributed by direct fields and unaliased fragments merge
only when they are structurally compatible:

- same output key;
- same resolved field or relation;
- same scalar type and nullability for scalar fields;
- compatible clauses, bindings, directives, and cardinality;
- compatible subselections for object or relation results.

Inferred selection cardinality participates in this compatibility check. Two
otherwise identical selections do not merge when one resolves to an array and
the other to a nullable object.

Scalar selections from multiple fragments therefore merge when they name the
same output key and resolve to the same catalog field:

```dsql
fragment UserIdentity on users {
  id
  name
}

fragment UserLabel on users {
  id
  name
}

query Users {
  users {
    ...UserIdentity
    ...UserLabel
  }
}
```

The result still contains one `id` field and one `name` field. Aliased scalar
fields follow the same rule by output key:

```dsql
fragment UserDisplayA on users {
  display_name: name
}

fragment UserDisplayB on users {
  display_name: name
}
```

Both fragments contribute the same `display_name` output from the same resolved
field, so they are compatible. This is invalid:

```dsql
fragment UserDisplayName on users {
  display_name: name
}

fragment UserDisplayEmail on users {
  display_name: email
}
```

Both fragments produce `display_name`, but they resolve to different catalog
fields.

If two selections produce the same output key but differ in field resolution,
clauses, ordering, limits, variable bindings, or other structural semantics, the
compiler reports a merge conflict. The compiler should not invent a merge
strategy for divergent selections. Users must alias either the conflicting
fields or the fragment spread.

Directive metadata participates in compatibility through the directive system.
If two merged selections carry the same non-repeatable directive with different
arguments, the compiler reports a directive conflict unless that directive
definition later declares a specific merge rule. Repeatable directives preserve
source order in checked metadata.

```dsql
query Users {
  users {
    ...RecentPosts($recent_after <- $after)
    ...PopularPosts($min_score <- $score)
  }
}
```

This is invalid because both fragments contribute a `posts` relation with
different clauses and bindings. The user can fix it by aliasing the spread, as
above, or by aliasing the relation fields inside the fragments.

## Comments

Line comments use `#`.

```dsql
# Fetch active users.
query Users {
  users {
    id # primary key
    name
  }
}
```

Comments are not part of the result shape.

## Output Shape

The output shape follows selection order.

```dsql
query Users {
  users {
    id
    display_name: name
    posts {
      title
    }
  }
}
```

Conceptual JSON shape:

```json
{
  "users": [
    {
      "id": 1,
      "display_name": "Ada",
      "posts": [
        {
          "title": "Hello"
        }
      ]
    }
  ]
}
```

## Common Fetch Patterns

The core query language should stay optimized for nested API read shapes. These
patterns should remain easy to express and should guide syntax and codegen
decisions.

### Latest Or First Related Row

Relation clauses can fetch a small ordered slice of related data.

```dsql
query Users {
  users(where .name like $$ limit 10) {
    id
    name

    latest_post: posts(order by created_at desc limit 1) {
      id
      title
      created_at
    }
  }
}
```

The literal `limit 1` makes `latest_post` a nullable object. The `order by`
clause chooses the latest matching row; without it, the selected row would be
unspecified. The same cardinality rule applies to root selections.

### Top-N Related Rows

Nested top-N collections are first-class relation selections, not a separate
feature.

```dsql
query Users {
  users(limit 10) {
    id
    name

    posts(order by created_at desc limit 3) {
      id
      title
      created_at
    }
  }
}
```

Generated SQL should avoid accidental full aggregation of large related tables
when relation limits are present.

### Output Renaming

Aliases are the normal way to shape API-friendly output names.

```dsql
query Users {
  users(limit 10) {
    id
    display_name: name

    latest_post: posts(order by created_at desc limit 1) {
      name: title
      published_at: created_at
    }
  }
}
```
