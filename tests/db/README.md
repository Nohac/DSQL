# Database Integration

Database tests are optional and skipped unless `DSQL_TEST_DATABASE_URL` is set.

```sh
DSQL_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/imdb cargo test -p dsql-tests
```

The integration path should eventually cover:

- applying setup SQL
- introspecting metadata
- compiling `.dsql` fixtures
- running generated SQL
- asserting JSON output shape
