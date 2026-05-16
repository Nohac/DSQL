# Directives

Status: unfinished.

Directives attach structured metadata to declarations, selections, fields, or
fragments.

## Intended Role

```dsql
query Users {
  users @plan.strategy(name: "batch") {
    id
  }
}
```

Possible directive categories:

- planning hints
- policy requirements
- code generation metadata
- frontend metadata
- provider-specific extensions

## Conditional Shape

Conditional includes are a possible future directive-like feature for API
exploration and generated endpoint variants.

```dsql
query Users {
  users(limit 10) {
    id
    name

    posts(if $$include_posts) {
      id
      title
    }
  }
}
```

Open questions:

- Whether conditional includes belong in directives, relation clauses, or code
  generation metadata.
- Whether disabled selections are omitted from the result shape or returned as
  `null`/empty collections.
- How conditional selections interact with generated TypeScript result types.

## Open Questions

- Exact directive name syntax.
- Whether namespaces use dotted names such as `@ui.table`.
- Which directive locations are valid.
- How directive schemas are registered.
- Whether unknown directives are errors or preserved as extension metadata.
- Which directives affect query semantics and which are metadata-only.

Directives must not become textual macros. Any directive that affects behavior
should be represented as structured syntax and resolved through language or
provider rules.
