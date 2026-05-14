# Project Fixtures

Use this directory for full project layouts when tests need to exercise config discovery and project loading.

For IMDb, use:

```text
tests/projects/imdb/dsql/dsql.toml
tests/projects/imdb/dsql/schema/
tests/projects/imdb/queries/
```

The schema can be copied or symlinked from `tests/schema/imdb` depending on what the test harness supports.

