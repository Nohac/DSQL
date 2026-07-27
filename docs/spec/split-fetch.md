# Split Fetch

Status: RFC.

Split fetches let a nested relation branch become an independently fetchable
unit. This is mainly useful for nested pagination, deferred loading, and
frontend interaction where a child collection changes without the parent query
needing to be refetched.

This is not only an N+1 mitigation feature. It also addresses coarse refetch
granularity and overfetching for nested result shapes: when one nested relation
branch needs another page or a refresh, the client should not have to refetch
every parent row and every already-hydrated nested branch just to update that one
child collection.

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

## Incremental Query Handoff

A master query can hydrate an initial nested branch and provide the identity and
context required for that branch to become independently fetchable later.

This is useful for framework flows such as server-side rendering a page with
users and the first page of posts, then letting each user's posts collection
paginate or refetch independently on the client.

The important behavior is that after the master query has provided parent
identity, each child relation can live as its own cache entry and endpoint. A
single user's posts can fetch the next page without rerunning the full
`UsersPage` query and without refreshing posts for every other user.

```dsql
fragment UserPosts on users @fetchable(key: "userPosts") {
  posts(first $$first after $$after) {
    id
    title
    created_at
  }
}

query UsersPage {
  users(limit $$limit) {
    id
    name
    ...UserPosts
  }
}
```

The master query can include the first `posts` page. Codegen can also expose
`UserPosts` as a child fetch target that accepts the parent user identity and
its own pagination params.

Possible generated TypeScript use:

```ts
const users = useQuery(UsersPageOperation, {
  params: { limit: 20 },
});

const posts = useQuery(UserPostsOperation, {
  input: {
    parent: users.data.users[index],
  },
  params: {
    first: 10,
    after: cursor,
  },
});
```

The child query key should include the fetchable unit, parent identity, local
params, and any required context.

```ts
["dsql", "UserPosts", { user_id: user.id, first: 10, after: cursor }]
```

The generated child fetch should target the relation scope directly instead of
refetching the full parent query.

Conceptual child correlation:

```sql
where posts.user_id = $parent_user_id
  and exists (
    select 1
    from users
    where users.id = posts.user_id
      and <active users filters>
  )
```

The generated child operation must derive its correlation through the filtered
parent source, not trust the supplied parent identity by itself. For a nested
handoff, it re-verifies every source in the complete parent chain under the
filters that an equivalent inline selection would apply. A parent row that is
no longer readable therefore cannot be used to reach its child rows.

## Query-Scoped Fragment Handles

Fetchable fragments may need to be context aware. The same fragment source can
be spread from different query paths, and each use can imply different parent
identity, result path, trusted context, conditionally filtered fields, or handoff
metadata. Because of that, generated runtime handles may need to be scoped to
the query/path where the fragment is used.

Possible generated TypeScript shape:

```ts
const users = useQuery(UsersPage, {
  params: { limit: 20 }
});

const posts = useFragment(UsersPage.fragments.UserPosts, users.data.users[0], {
  params: { first: 10 }
});
```

Here `UsersPage.fragments.UserPosts` is not merely the source fragment. It is a
query-scoped handle that knows:

- the owning query
- the result path where the fragment was spread
- the parent identity required to fetch the child branch
- local params for the child branch
- required trusted context and applicable filter metadata

This keeps split fetches tied to a concrete handoff contract. A plain
`UserPosts` handle may still be useful for truly standalone fragments, but it
should not be the default assumption for fragments that depend on parent data.

An independently executed child preserves the operation-wide filter assignments
and trusted-context requirements of the operation that produced its handoff.
Source-local assignments on the child source remain part of that checked
handoff. At execution time, the server boundary re-binds current trusted context
instead of replaying values captured by the parent fetch. A client cannot alter
the assignments, requirements, or checked parent authorization chain through
split-fetch params.

Handoff metadata describes that compiler-checked chain so integrations can
identify and cache the child operation. It is not an authorization claim from
the client: enforcement is encoded in the generated child operation and
re-evaluated server-side.

## Handoff Metadata

Codegen should expose enough metadata for a framework adapter to hydrate child
queries from a master query and later refetch them independently.

Possible shape:

```json
{
  "handoffs": [
    {
      "name": "UserPosts",
      "fragment": "UserPosts",
      "parent_path": "users[]",
      "parent_identity": ["id"],
      "relation": "posts",
      "child_params": ["first", "after"],
      "parent_bindings": {
        "user_id": "users[].id"
      },
      "authorization_chain": ["users", "users.posts"]
    }
  ]
}
```

This shape is illustrative. The important contract is that generated clients can
derive a stable child query key and fetch target from data already returned by
the master query.

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
- How cache keys are generated.
- How SSR hydration should seed child query caches from master query results.
- Whether fetchable fragments should be explicit declarations, directives, or
  inferred from relation selections.
