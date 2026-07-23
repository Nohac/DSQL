# Relationship Naming

Status: in progress.

Relationship names are catalog-driven. The language does not singularize,
pluralize, or otherwise rewrite table names by itself.

## Provider Default

If a provider relationship points to `public::posts`, the default selectable
relation name is `posts`.

```dsql
query Users {
  users {
    posts {
      id
    }
  }
}
```

Explicit directional relationship names, provider-edge hiding, conflict rules,
and manual joins are defined by [Catalog Overlays](catalog-overlays.md).
Hide-plus-add is the supported authored naming mechanism: it changes the
exposed catalog field while preserving any independently matching provider
proof.

## Overlay Relationships

Catalog overlays allow named relationships that reuse an inferred foreign-key
path or declare an ordered manual join.

Conceptually:

```text
assignee = users->assignee_id
reviewer = users->reviewer_id
```

Those names are selected directly:

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

This provides stable, user-owned relationship names while keeping the raw
`[schema::]table->edge` selector available as an explicit lower-level
reference.

Benefits include:

- avoiding physical foreign-key column names in ordinary queries;
- preserving established API relationship names during migration;
- giving views and other non-FK-backed objects relationship metadata;
- allowing tooling to generate suggested overlays for review; and
- reducing query churn when database details change but the desired API shape
  does not.

## Global Rewrite Rules To Consider

Version 1 catalog overlays deliberately require per-object declarations.
Project configuration may eventually support global relationship naming rewrite
rules for broad conventions.

Possible use cases:

- normalize provider relationship names across a schema;
- inspect foreign-key column names;
- match an established API naming convention; and
- keep generated output stable when database constraint names are noisy.

Open questions for global rules:

- whether rules apply globally or per schema;
- which provider facts rules may inspect;
- how generated suggestions become reviewed authored metadata;
- how rule changes interact with explicit overlays; and
- whether output keys follow rewritten names or require explicit aliases.

Global rewrite rules must not introduce a second conflict or provenance model.
If added, they must normalize into the effective-catalog composition defined by
[Catalog Overlays](catalog-overlays.md).
