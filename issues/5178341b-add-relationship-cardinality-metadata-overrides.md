# Add relationship cardinality metadata overrides

**ID:** 5178341b | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Allow project metadata to override relationship cardinality for generated result
types and runtime metadata.

## Context

DSQL should default relationship selections to list/array results unless project
metadata explicitly says otherwise. PostgreSQL foreign keys and SQL joins do not
provide a first-class runtime guarantee that a selected relationship is
semantically one object in the shape TypeScript generation needs.

Inferring object-vs-array from foreign-key direction is easy to make unsound,
especially with project schemas, views, nullable foreign keys, partial
uniqueness, or non-conventional data.

Potential metadata shape:

```toml
[relations.title.kind_type]
cardinality = "one"

[relations.title.movie_info]
cardinality = "many"
```

## Done When

- Project metadata can declare relationship cardinality overrides.
- Generated TypeScript and runtime metadata use the override consistently.
- The default remains conservative and sound.
- Invalid override targets or values produce diagnostics.
- Tests cover default-many behavior and explicit one/many overrides.
