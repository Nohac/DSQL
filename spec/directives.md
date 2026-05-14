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
