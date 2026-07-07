# Programmatic Resolvers

Status: consideration.

Programmatic resolvers would let project metadata add fields, relationships, or
entire virtual entities that are not directly backed by SQL catalog objects.
DSQL would still typecheck and codegen them, but the generated application layer
would call user-owned code to resolve those values.

This is not part of the current query language. It is a possible extension for
cases where a project needs to mix SQL-shaped data with domain behavior that
cannot or should not live in the database.

## Field Resolvers

A resolver could expose a synthetic field on an existing entity.

```dsql
query Users {
  users {
    id
    name
    my_custom_function
  }
}
```

The catalog/provider metadata would describe `my_custom_function` as a resolved
field on `users`, including its output type and the source context it needs. The
code generator could then emit a typed function stub that receives the selected
row context.

Example generated contract shape:

```ts
export async function resolveUsersMyCustomFunction(ctx: {
  parent: { id: number; name?: string };
  context: AppContext;
}): Promise<string | null> {
  // user implementation
}
```

The exact generated API should be provider-specific. DSQL should describe the
resolver contract in metadata rather than hardcoding one runtime model.

## Resolver Edges

Resolvers could also expose custom edges between entities, similar to a
relationship whose backing implementation is user code instead of a foreign key.

```dsql
query Users {
  users {
    id
    recommended_posts {
      id
      title
    }
  }
}
```

This could model search results, permission-filtered collections, external
service lookups, cached projections, or relationships that cross data sources.

## Virtual Entities

A project might define an entire virtual entity whose rows are produced by user
code but can still participate in DSQL selection and type generation.

```dsql
query Dashboard {
  dashboard_widgets {
    id
    label
    current_value
  }
}
```

This should be treated carefully. Once virtual entities can be filtered,
ordered, paginated, or joined with SQL-backed entities, the resolver contract
needs clear capability metadata so query authors know what is actually
supported.

## Batching

Programmatic resolvers can easily introduce N+1 behavior. Metadata should be
able to describe whether a resolver is scalar, per-parent, or batchable.

Possible resolver modes:

- `single`: called once for one parent row.
- `batch`: called once for many parent rows and returns values keyed by parent
  identity.
- `root`: called once as a root query source.

Batchable resolver contracts should receive all parent IDs or selected parent
context needed to resolve the field or edge in one call.

Example batch contract shape:

```ts
export async function resolveUsersRecommendedPosts(ctx: {
  parents: Array<{ id: number }>;
  context: AppContext;
}): Promise<Map<number, Array<{ id: number; title: string }>>> {
  // user implementation
}
```

## Metadata Direction

Resolver metadata probably needs to describe:

- Where the resolver is mounted.
- Whether it is a field, edge, or root virtual entity.
- The input context required from the parent selection.
- The output type or output entity shape.
- Nullability and cardinality.
- Whether it supports batching.
- Whether it supports filtering, ordering, pagination, or only plain selection.
- Which runtime/codegen provider owns the implementation contract.

## Codegen Notes

DSQL should not execute programmatic resolvers itself. It should expose enough
metadata for a provider to generate typed implementation stubs and wire calls
into the generated endpoint or framework adapter.

Generated SQL and generated resolver calls should remain separate phases where
possible. The SQL phase can hydrate the parent context required by resolver
metadata; the resolver phase can then compute synthetic fields or edges.

## Risks

- N+1 behavior if resolver batching is not explicit.
- Hard-to-debug performance if resolver calls are hidden behind normal field
  selection.
- Type drift if resolver implementation signatures are not generated from the
  same metadata used by query checking.
- Ambiguous semantics for filtering, ordering, or pagination over resolver
  fields.
- Provider lock-in if resolver metadata encodes one runtime too directly.

Open questions:

- Should resolver fields require explicit syntax, or should metadata-backed
  resolver names look exactly like catalog fields.
- Whether resolver edges can be used in predicates.
- Whether resolver output can be spread through fragments.
- How much context a resolver can request without forcing overfetching.
- Whether resolver batching should be mandatory for to-many edges.
- How LSP hover/completion should distinguish SQL-backed and resolver-backed
  fields.
