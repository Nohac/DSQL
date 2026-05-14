# Grouping

Status: unfinished.

Grouping returns aggregate rows grouped by one or more fields.

## Table Grouping

```dsql
query PostsByStatus {
  posts.group_by(status) {
    status
    count
    latest_created_at: max(created_at)
  }
}
```

Meaning:

- Each output row represents one `status` group.
- `status` is selectable because it is a grouping key.
- `count` and `max(created_at)` are aggregate outputs.

## Nested Grouping

Grouping may also be useful under relations.

```dsql
query Users {
  users {
    id
    posts.group_by(status) {
      status
      count
    }
  }
}
```

Meaning:

- For each user, return related posts grouped by `status`.
- The grouping scope is the related `posts` collection for that user.

Open questions:

- Exact group syntax.
- How grouping composes with `where`, `order by`, and pagination.
- Whether non-aggregate selections must always be grouping keys.
- Whether nested grouping should require a split/batched plan.
- How grouped output keys should be named.

