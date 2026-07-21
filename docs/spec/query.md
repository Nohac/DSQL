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

A query header may refine inferred public inputs and assign named filters
recursively across the operation:

```dsql
query Administration(
  $$includeDeleted = false
  filter SoftDelete when not $$includeDeleted
) {
  users {
    id
  }
}
```

Operation assignments, source-local overrides, and enforcement conditions are
defined in [Filters And Access Rules](policies.md#operation-wide-assignments).
Input refinements are defined in
[Definition Input Refinements](variables.md#definition-input-refinements). The
parenthesized header contains one or more input refinements or `filter`
assignments separated by normal DSQL whitespace. Variables remain inferred from
their usage; header entries only add nullability or defaults to those inferred
contracts. Directives, when present, follow the closing parenthesis. Empty
parentheses are invalid and omitted by the formatter.

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
is nullable, a clause suppresses the row, or an active filter removes it,
every field contributed through the flatten inherits that absence and is
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
relationship is optional, a clause suppresses it, or an active filter
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
- `or` branches do not prove uniqueness unless every branch constrains the
  complete key to the same fixed values. Covering the same key columns with
  different values can return one row per alternative and is not at-most-one.
- A conditionally omitted predicate does not participate. An optional non-null
  variable with a non-null default may participate because its predicate
  remains present in every compiled variant. A nullable variable cannot,
  because an explicit `null` structurally removes its predicate atom. See
  [Variables](variables.md#nullable-predicate-uses).
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
unspecified. An `offset`, a zero-valued runtime limit, or an active filter may
turn a proven singular result into `null`, but cannot broaden it back into a
collection.

Inferred cardinality is part of the semantic result shape. Planning, SQL
generation, result metadata, generated API types, flattening, fragment merging,
and editor services must consume the same resolved cardinality rather than
re-deriving it independently.

The SQL artifact for an at-most-one root still returns exactly one protocol
row. An object-valued root projects its wrapper key as `null` when no source row
matches. A flattened at-most-one root projects one row with each contributed
field set to `null` when no source row matches. This envelope is an artifact
transport detail; generated result types expose the nullable object or nullable
flattened fields described above, not the envelope itself.

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

- `filter <filter-name> [when <boolean-value>]`
- `where <predicate>`
- `order by <field> <direction>[, ...]`
- `limit <integer>`
- `offset <integer>`

Active filters form the readable source before these explicit clauses are
evaluated. `filter` assigns the desired state of one named filter; `where`,
`order by`, `limit`, and `offset` then operate on the resulting readable source.
Canonical clause order is zero or more `filter` assignments followed by
`where`, `order by`, `limit`, and `offset`.

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

Comparison, `like`, and `and`/`or` composition are implemented. Membership,
unary `not`, dedicated null tests, and SQL-style `exists` below are accepted
predicate extensions but are not part of the current implementation.

Core predicate operators:

```text
==
!=
>
>=
<
<=
in
not in
```

Text-like fields additionally support `like`. Predicates may be combined with
boolean `and`, `or`, and unary `not`.

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
  users(where (.active == true or .trial == true) and .deleted_at is null) {
    id
  }
}
```

The predicate grammar should follow this shape:

```text
predicate_expr = or_expr
or_expr        = and_expr ("or" and_expr)*
and_expr       = unary_expr ("and" unary_expr)*
unary_expr     = "not" unary_expr | primary_expr
primary_expr   = predicate | "(" predicate_expr ")"
predicate      = comparison
               | membership
               | null_test
               | exists_predicate
               | scalar_aggregate_predicate
               | boolean_value
comparison     = field_path comparison_operator value_or_field_path
membership     = field_path ("in" | "not" "in") collection_value
null_test      = field_path "is" ["not"] "null"
exists_predicate = "exists" collection_source
boolean_value  = boolean_literal | public_boolean_variable | context_boolean
```

Each side of a boolean operator must be a predicate expression. Bare field paths
are not valid boolean predicates by themselves. Boolean literals and boolean-
typed public or trusted values are valid predicate atoms, which permits forms
such as `$:is_admin or .owner_id == $:user_id` and `not $$includeDeleted`.

`not` binds more tightly than `and`, which binds more tightly than `or`.
Parentheses may always make the intended grouping explicit.

### Membership

Membership accepts a typed literal collection, a public array input, or trusted
context collection:

```dsql
query ActiveOrInvitedUsers {
  users(where .status in ["active", "invited"]) {
    id
  }
}

query UsersById {
  users(where .id in $$userIds) {
    id
    name
  }
}
```

The variable `$$userIds` is inferred as an array of the logical type of `.id`.
Filter declarations use the same predicate language and may consume trusted
collections:

```dsql
filter AllowedTenants on {
  .tenant_id: uuid
} {
  apply where true
  where .tenant_id in $:tenant_ids
}
```

Exclusion uses `not in`:

```dsql
documents(where .state not in ["deleted", "archived"]) {
  id
}
```

For an empty collection, `x in []` is false and `x not in []` is true.
Duplicate collection values do not change predicate truth. The collection
element type must be compatible with the field type.

Nullable collection elements are valid and follow PostgreSQL three-valued
membership semantics:

- `x in [x, null]` is true for a non-null matching `x`;
- `x in [y, null]` is unknown when non-null `x` does not equal `y`;
- `x not in [x, null]` is false;
- `x not in [y, null]` is unknown when non-null `x` does not equal `y`;
- a null left operand produces unknown for a non-empty collection;
- unknown excludes the row when used by `where`.

SQL lowering through an `IN` list, `ANY`/`ALL`, or another strategy must
preserve these results. Empty collections retain the explicit false/true
semantics above, including for a null left operand.

### Negation And Null Tests

Unary `not` negates a predicate expression while preserving SQL three-valued
logic:

```dsql
documents(where not (.embargoed and .owner_id != $:user_id)) {
  id
}
```

Null checks use dedicated syntax:

```dsql
users(where .deleted_at is null) {
  id
}

users(where .deleted_at is not null) {
  id
}
```

The equality spellings are also valid null-test aliases:

```dsql
users(where .deleted_at == null) {
  id
}

users(where .deleted_at != null) {
  id
}
```

`== null` has exactly the semantics of `is null`, and `!= null` has exactly the
semantics of `is not null`; neither lowers to ordinary SQL equality or
inequality with `NULL`. This alias applies only when `null` is written as the
source literal. A comparison to a variable that contains `NULL` at runtime uses
ordinary PostgreSQL three-valued equality or inequality. The formatter
preserves the accepted spelling rather than forcing one style.

### Existence

Related-row existence uses SQL-style prefix syntax:

```dsql
projects(where exists .project_users) {
  id
}
```

The relation is observed through the filtered logical view, so a row filter or
relation-field filter may make `exists` false. The source may carry clauses:

```dsql
projects(
  where exists .project_users(where .user_id == $:user_id)
) {
  id
}
```

It may also be an unrelated qualified table source correlated through normal
scope prefixes:

```dsql
projects(
  where exists public::administrators(
    where .user_id == $:user_id
      and .tenant_id == ..tenant_id
  )
) {
  id
}
```

`not exists <source>` follows from unary `not`; it is not a separate aggregate
function. Query-authored sources observe the filtered logical view, while
filter-authored sources use the raw rule-evaluation boundary.

The initial `exists` source permits named `filter` assignments and `where`.
`order by` is irrelevant to existence, while `limit` and `offset` would add
slicing semantics; all three are diagnostics in the initial contract.

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

### Filter Assignment

`filter` assigns the desired state of a named filter for the current collection
source. Without `when`, it assigns `true` and therefore activates a manual
filter:

```dsql
query PublishedPosts {
  posts(filter Published) {
    id
  }
}
```

The assigned state may be a row-independent public or trusted boolean:

```dsql
query Users {
  users(filter SoftDelete when not $$includeDeleted) {
    id
    deleted_at
  }
}
```

The assigned boolean must be non-null. A public variable used by `when` cannot
carry a nullable `?` refinement; this is a compile-time diagnostic rather than a
third assignment state. Use a non-null boolean default when callers may omit the
value.

This assignment is `false` when deleted rows are requested. A static opt-out
uses the ordinary boolean literal:

```dsql
users(filter SoftDelete when false) {
  id
  deleted_at
}
```

The assignment controls desired state, while `apply where` may still enforce
the filter from trusted context. Unknown names, nonmatching filters, duplicate
assignments at one scope, and an assignment that can be false for `apply where
true` are diagnostics. A statically true assignment to that filter is merely
redundant. See [Query Filter Assignments](policies.md#query-filter-assignments).

## Values

Core literal values:

```dsql
123
12.5
true
false
null
"text"
[1, 2, 3]
["active", "invited"]
```

List literals are homogeneous typed collection values used by membership
predicates. Empty lists infer their element type from the field on the other
side of `in` or `not in`. General list-valued expressions and object literals
are outside the initial predicate contract. The empty object `{}` is reserved
as the identity default for a bounded dynamic predicate; it is not yet a
general query expression.

## Fragments

Fragments define reusable selection sets for a catalog target.

```dsql
fragment UserSummary(
  $$post_limit = 5
) on public::users {
  id
  name
  posts(limit $$post_limit) {
    id
  }
}

query Users {
  users {
    ...UserSummary
  }
}
```

The fragment target resolves through the same table-resolution rules as root
selections. A fragment spread is valid only when the fragment target matches the
current selection context. Like a query, a fragment may refine its inferred
public inputs in a parenthesized header before `on`; fragment headers do not
contain operation-wide filter assignments. See
[Definition Input Refinements](variables.md#definition-input-refinements).

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
variables spec. Named items bind individual inferred leaves. Bare `$` or `$$`
items lift the corresponding complete fragment input root, while a root mapping
such as `$$ <- $$namespace` moves that complete contract beneath a caller
namespace. See [Bound Definition Inputs](variables.md#bound-definition-inputs).

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
