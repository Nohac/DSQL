# Mutations

Status: consideration.

Mutations are not part of the current read-query language, but the syntax should
leave room for write operations later.

## Possible Shapes

```dsql
mutation CreateUser($name: string) {
  insert_users(value { name: $name }) {
    id
    name
  }
}
```

or:

```dsql
mutation CreateUser($name: string) {
  users.insert({ name: $name }) {
    id
    name
  }
}
```

Open questions:

- Whether mutations belong in dsql at all.
- How transactions are represented.
- How permissions are enforced.
- How return shapes are validated.
- Whether generated REST endpoints should support writes separately from the
  query language.

