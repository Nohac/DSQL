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

## Defined Relationship Aliases

Project metadata may eventually allow named relationships that alias an inferred
foreign-key path.

Example idea:

```text
assignee -> users::assignee_id
reviewer -> users::reviewer_id
```

Those names could then be selected directly:

```dsql
query Tasks {
  tasks {
    assignee {
      id
      name
    }

    reviewer {
      id
      name
    }
  }
}
```

This would provide stable, user-owned relationship names while keeping the raw
`[schema.]table::foreign_key` selector available as an explicit lower-level
reference.

Potential benefits:

- Avoid forcing query authors to use physical foreign-key column names
  everywhere.
- Preserve existing API relationship names when importing metadata from systems
  such as Hasura.
- Give views and other non-FK-backed objects relationship metadata.
- Let introspection generate suggested aliases that users can edit.
- Reduce query churn when database details change but the desired API shape does
  not.

Open questions:

- Whether aliases are scoped globally, per source table, or per schema/table.
- Whether an alias may point only to inferred FK paths or also to hand-authored
  join definitions.
- Whether alias selection output keys use the alias name by default.
- How alias conflicts with table names, columns, and inferred relations are
  reported.
- Whether generated aliases should be checked into project metadata or remain
  provider-owned.
