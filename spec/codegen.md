# Code Generation Metadata

Status: RFC.

dsql should be able to emit more than SQL. A checked query can also produce
metadata for application code generation.

## Possible Artifacts

- TypeScript result and variable types.
- Runtime validation schemas.
- Query helpers.
- Table column metadata.
- Form metadata.
- Cache key builders.
- Split-fetch helpers.
- Operation metadata JSON.

## Frontend Metadata

```dsql
query UsersTable($limit: int, $offset: int)
  @ui.table(total: "users_total")
{
  users(limit $limit offset $offset) {
    id @ui.column(hidden: true)
    name @ui.column(label: "Name")
    email @ui.column(label: "Email")
  }

  users_total: users.count()
}
```

Metadata directives should not change SQL semantics unless they are explicitly
defined as planning or policy directives.

## Identity Metadata

Generated metadata may need stable identity information:

- object identity fields
- result paths
- relation parent-child keys
- variables participating in cache identity
- policy context values participating in cache identity
- split-fetch cache keys

Open questions:

- Whether codegen metadata is directive-based, config-based, or both.
- How much metadata can be inferred from the catalog.
- How generated metadata refers to result paths.
- How provider-specific metadata is represented.
- Whether metadata output should be stable JSON, typed Rust structs, or both.

