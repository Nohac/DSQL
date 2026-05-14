# Relationship Naming

Status: consideration.

Relationship names are catalog-driven. The language does not singularize,
pluralize, or otherwise rewrite table names by itself.

## Current Rule

If a relationship points to `public.posts`, the default selectable relation name
is `posts`, unless catalog metadata provides another name.

```dsql
query Users {
  users {
    posts {
      id
    }
  }
}
```

## Rewrite Rules To Consider

Project configuration may eventually support relationship naming rewrite rules.
This would let a project normalize catalog-derived relationship names without
hardcoding naming behavior in the language.

Possible use cases:

- Rename relationships generated from foreign-key metadata.
- Resolve multiple relationships between the same two tables.
- Match existing API naming conventions.
- Keep generated output stable when database constraint names are noisy.

Open questions:

- Whether rewrite rules apply globally or per table/schema.
- Whether rules can inspect foreign-key column names.
- How conflicts are reported.
- Whether output keys follow rewritten relation names or require explicit
  aliases.
- How rewrite rules interact with schema-qualified relation references.
