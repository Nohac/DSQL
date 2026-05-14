# dsql Test Fixtures

This directory contains tracked fixtures for compiler, LSP, formatter, SQL generation, linting, and optional database integration tests.

The intended flow is:

1. Put a real catalog under `schema/imdb`.
2. Add `.dsql` files under `queries/valid` and `queries/invalid`.
3. Add snapshot tests that read these fixtures and write expected output under `snapshots`.
4. If a PostgreSQL database is available, run optional integration tests against it.

Normal tests do not require a running database. Database tests are gated by `DSQL_TEST_DATABASE_URL`.

```sh
cargo test -p dsql-tests
DSQL_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/imdb cargo test -p dsql-tests
```

Snapshots use `insta` and are stored in `snapshots`.

```sh
INSTA_UPDATE=always cargo test -p dsql-tests
```
