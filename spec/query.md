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
  public.users(where id > 100 limit 20) {
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

The default schema is configured by the project. If it is not configured, the
default schema is `public`.

An unqualified table reference resolves through the default schema. With the
default configuration, `users` resolves as `public.users`.

The output key for a schema-qualified table or relation is still the table name
unless an alias is provided. For example, `public.users` produces `users`.

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

## Relationship Ambiguity

A relation selection is ambiguous only when the catalog exposes more than one
relationship with the same selectable name from the same parent table.

For example, ambiguity can exist if project or catalog metadata exposes both of
these relationships from `public.users` under the selectable name `posts`:

```text
public.posts.author_id -> public.users.id  as posts
public.posts.editor_id -> public.users.id  as posts
```

Then this selection has no way to choose which foreign key to use:

```dsql
query Users {
  users {
    posts {
      id
    }
  }
}
```

This is a catalog naming problem, not a syntax problem. The user should resolve
it through distinct relationship names in catalog/project metadata, or by using
whatever alias/relationship-disambiguation syntax the catalog provider exposes.

If the catalog only exposes one relationship named `posts`, the selection is not
ambiguous.

## Clauses

Selection clauses are written inside parentheses after a root or relation
selection.

```dsql
query Posts {
  posts(where created_at >= "2026-01-01" order by created_at desc limit 20) {
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
  users(where active == true) {
    id
    name
  }
}
```

Predicates resolve field names against the selected table or relation target.

Core predicate operators:

```text
==
!=
>
>=
<
<=
```

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
