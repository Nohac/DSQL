# Schema Fixtures

Schema fixtures should use the same metadata format the project loader consumes.

For the IMDb catalog, place metadata under:

```text
tests/schema/imdb/
```

Expected shape:

```text
tests/schema/imdb/type_map.yaml
tests/schema/imdb/public/<table>.yaml
tests/schema/imdb/<other_schema>/<table>.yaml
```

This lets compiler tests exercise the real metadata loader instead of a separate hardcoded test catalog.

