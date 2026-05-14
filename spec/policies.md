# Policies And Permissions

Status: RFC.

Policies describe implicit constraints and access rules over queries. They may
come from project configuration, provider metadata, generated metadata, or
eventually dsql declarations.

## Policy Use Cases

- Soft-delete filtering.
- Tenant scoping.
- Row visibility rules.
- Field visibility rules.
- Relation traversal permissions.
- Policy override requirements.

## Soft Delete

If a table has a configured soft-delete policy, a query can implicitly exclude
deleted rows.

```dsql
query Users {
  users {
    id
    name
  }
}
```

Conceptual filter:

```text
users.deleted_at is null
```

The same policy can apply to relation selections.

## Context Values

Policies may require runtime context values distinct from user query variables.

```text
$ctx.user_id
$ctx.tenant_id
```

User queries should not set `$ctx` values directly. Hosts provide them.

## Capabilities

Permissions should be modeled as capabilities rather than only roles.

```dsql
query DeletedUsers @requires(capability: "users.read.deleted") {
  users @policy.includeDeleted {
    id
    deleted_at
  }
}
```

The directive does not grant access by itself. It declares a requirement that
must be checked by the host or policy system.

Open questions:

- Whether policy declarations belong in dsql or only project/provider config.
- Exact override syntax.
- How policy-derived filters are surfaced in debug output.
- How field-level permissions affect result shape and generated types.
- How policies interact with fragments and split fetches.

