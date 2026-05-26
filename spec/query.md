# Query Language

Status: in progress.

dsql is a domain specific query language for describing read query shape over a
relational catalog. The language is nested like GraphQL where it describes
result shape, and SQL-like where it filters, orders, and slices data.

## Example

```dsql
fragment PostSummary on public.posts {
  id
  title
}

query UsersWithPosts {
  public.users(where .id > 100 limit 20) {
    id
    display_name: name
    posts(order by created_at desc limit 5) {
      ...PostSummary
    }
  }
}
```

This describes a result rooted at `users`. `id` and `name` resolve to columns on
`public.users`. `posts` resolves through catalog relationship metadata from
`public.users` to `public.posts`. The fragment is valid because it targets the
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

Qualified names use one schema segment and one object segment:

```dsql
public.users
other_schema.users
```

Relationship paths may add a foreign-key selector after the table reference:

```dsql
public.users::assignee_id
users::reviewer_id
```

The part before `::` is still the table reference. The part after `::`
selects which foreign-key path connects the current table to that relation
target.

The default schema is configured by the project. If it is not configured, the
default schema is `public`.

An unqualified table reference resolves through the default schema. With the
default configuration, `users` resolves as `public.users`.

The output key for a schema-qualified table or relation is still the table name
unless an alias is provided. For example, `public.users` produces `users`.

The output key for a relation with a foreign-key selector is also the table
name unless an alias is provided. For example, `users::assignee_id` produces
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

Aliases are used when two selections would produce the same output key.

```dsql
query Users {
  public_users: public.users {
    id
  }

  other_users: other_schema.users {
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

Qualified roots are explicit:

```dsql
query Users {
  other_schema.users {
    id
  }
}
```

Relationship names are catalog-driven. The language does not singularize,
pluralize, or otherwise rewrite them by itself.

Relation result cardinality also comes from catalog metadata. Relation
selections are collection-valued unless catalog constraints prove at-most-one
cardinality, for example through a unique or primary-key foreign-key column. See
[Catalog Metadata](catalog-metadata.md#relation-cardinality).

Qualified relation references are allowed:

```dsql
query Users {
  public.users {
    public.posts {
      id
    }
  }
}
```

The output key for `public.posts` is still `posts`.

If multiple foreign-key paths connect the current table to the same relation
target, the relation must include a foreign-key selector.

```dsql
query Tasks {
  tasks {
    users::assignee_id {
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

Schema qualification and foreign-key selectors may be combined:

```dsql
query Tasks {
  tasks {
    public.users::reviewer_id {
      id
    }
  }
}
```

When only one foreign-key path connects the current table to the relation
target, the selector may be omitted.

## Relationship Ambiguity

A relation selection is ambiguous when it names a relation target but multiple
foreign-key paths connect the current table to that target.

For example, ambiguity exists if `public.tasks` has both of these foreign keys
to `public.users`:

```text
public.tasks.assignee_id -> public.users.id
public.tasks.reviewer_id -> public.users.id
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

The selection must disambiguate the foreign-key path:

```dsql
query Tasks {
  tasks {
    users::assignee_id {
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
    assignee: users::assignee_id {
      id
    }

    reviewer: users::reviewer_id {
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

`limit` and `offset` slice a collection.

```dsql
query Page {
  posts(order by created_at desc limit 20 offset 40) {
    id
    title
  }
}
```

`limit` and `offset` values are integer-compatible.

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
fragment UserSummary on public.users {
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

This produces the normal relation output shape unless another feature explicitly
unwraps single-row relation selections.

Open question:

- Whether a relation with `limit 1` should have an opt-in singular output shape.

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
