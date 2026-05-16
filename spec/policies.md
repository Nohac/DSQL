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

## Global Context

Policies often need host-provided values such as the current user, tenant, role,
or request-scoped permissions. These values are distinct from user-provided query
variables.

```dsql
context user_id: uuid
context tenant_id: uuid
context role: text
```

Context values are referenced with the `$:<name>` form.

```dsql
$:user_id
$:tenant_id
$:role
```

Rules:

- `$:<name>` values are provided by the host/runtime, not by public query input.
- `$:<name>` values must be declared or provided by project/provider metadata before
  use.
- Generated metadata should list every context value required by a query.
- `$:<name>` should be valid in policy/default-filter expressions and may also be
  valid in explicit query predicates when the project allows it.

The symbol roles should stay separate:

- `#` is comments.
- `@` is directives.
- `$` and `$$` are user-provided query inputs.
- `$:<name>` is host-provided context.

This avoids reserving a normal variable name such as `$ctx`.

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

## Default Filters

Default filters are implicit predicates that apply to matching tables or
relations. They are useful for soft-delete, tenant scoping, and other structural
constraints that should be present across many queries.

```dsql
default filter SoftDelete on {
  .deleted_at: timestamptz
} where .deleted_at is null
```

The `on` shape matches catalog targets that have compatible fields. The filter
is applied to every matching root or relation selection unless explicitly
disabled by policy rules.

Tenant scoping can be modeled as a default filter with host context:

```dsql
default filter TenantScope on {
  .tenant_id: uuid
} where .tenant_id == $:tenant_id
```

Default filters compose with local query predicates using `and`.

```text
final where =
  default filters
  and policy checks
  and explicit query where
  and bounded dynamic filters
```

Open questions:

- Whether default filters are opt-out or opt-in by project configuration.
- Exact override syntax for privileged queries.
- Whether shape matching can require indexes or other catalog metadata.
- How default filters apply to views that do not expose underlying table
  metadata directly.

## Row Policies

Row policies describe access checks for rows. They can target a concrete catalog
object or a structural shape.

```dsql
policy RecordingRead on recording {
  check where .project.project_users.user_id == $:user_id
}
```

This is equivalent to a nested Hasura-style permission tree:

```yaml
recording:
  project:
    project_users:
      user_id:
        _eq: X-Hasura-User-Id
```

In DSQL, relationship traversal uses scoped predicate paths. To-many
relationships keep their normal existential predicate semantics.

Policies may compose with default filters:

```dsql
policy RecordingRead on recording {
  include SoftDelete
  check where .project.project_users.user_id == $:user_id
}
```

The policy system should expose applied row policies in generated metadata and
debug output so implicit predicates are not invisible.

## Field Visibility

Field and relation access should preserve result shape by default. Instead of
producing a different schema per role, a hidden field should return `null` or an
empty relation value.

```dsql
policy UserPrivacy on users {
  field email visible when $:role == "admin" or .id == $:user_id
  field phone visible when $:role == "admin"
}
```

Semantics:

- The field remains selectable and present in generated result types.
- If the visibility predicate is true, the real value is returned.
- If the visibility predicate is false, scalar and to-one fields return `null`.
- To-many relations should return an empty collection when hidden.
- Generated types become nullable when an active policy can hide the field.

Conceptual SQL lowering for a scalar field:

```sql
case
  when ($role = 'admin' or users.id = $user_id) then users.email
  else null
end as email
```

Relation visibility follows the same stable-shape rule:

```dsql
policy UserPrivacy on users {
  relation sessions visible when $:role == "admin" or .id == $:user_id
}
```

If `sessions` is hidden, the output value is `[]` for a to-many relation and
`null` for a to-one relation.

LSP and code generation should surface this:

- Hover on a field should indicate when a policy can mask it.
- Generated metadata should record `visibility: conditional`.
- Generated result types should reflect policy-driven nullability.
- Source maps should preserve the policy source span for diagnostics and debug
  output.

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
- Whether field visibility policies may reference relationship paths or only
  local/root fields.
- Whether policy syntax should use `field ... visible when`, directives, or a
  more compact declaration form.
