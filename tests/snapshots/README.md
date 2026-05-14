# Snapshots

Snapshot files live here and are managed by `insta`.

Suggested snapshot groups:

- parse diagnostics
- semantic diagnostics
- formatter output
- query plans
- generated PostgreSQL SQL
- LSP completion, hover, and semantic token output

Before snapshotting new output kinds, normalize unstable values such as absolute paths or generated aliases if needed.

```sh
INSTA_UPDATE=always cargo test -p dsql-tests
```
