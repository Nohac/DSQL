# Split Fetch

Status: RFC.

Split fetches let a nested relation branch become an independently fetchable
unit. This is mainly useful for nested pagination, deferred loading, and
frontend interaction where a child collection changes without the parent query
needing to be refetched.

## Relation Split Fetch

```dsql
query UsersWithPosts {
  users(limit 20) {
    id
    name
    posts(first 5) @splitFetch(name: "UserPosts") {
      id
      title
    }
  }
}
```

The initial query may include the first `posts` page. A generated split fetch can
later fetch more posts for one or more parent users.

## Fetchable Fragments

Fragments may also become fetchable units.

```dsql
fragment UserPosts on users @fetchable(key: "userPosts") {
  posts(first $first after $after) {
    id
    title
  }
}
```

Used in a query:

```dsql
query Users {
  users {
    id
    name
    ...UserPosts
  }
}
```

## Identity And Stitching

A split fetch needs stable identity metadata:

- parent object
- parent key
- child object
- child key
- relation foreign key
- output path
- required variables and context values

If an identity field is not selected by the user, the implementation may need to
fetch it internally without exposing it in the result.

Open questions:

- Whether split fetch syntax is directive-based or declaration-based.
- How split fetches inherit variables from the parent operation.
- Whether a split fetch can be executed independently of the original query.
- How policy/context requirements are carried into split fetches.
- How cache keys are generated.

