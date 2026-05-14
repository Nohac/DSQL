# Metadata Sources

Status: consideration.

dsql should support multiple metadata sources instead of assuming PostgreSQL
introspection is the only way to build the catalog.

## Sources To Consider

- PostgreSQL introspection.
- Hardcoded or generated project metadata.
- Drizzle schema metadata.
- ORM metadata from other ecosystems.
- Hand-authored schema files.

## Drizzle

Drizzle can already describe tables, columns, relations, and application-level
types. A Drizzle provider could let dsql reuse that metadata rather than
requiring database introspection.

Potential benefits:

- Better application-level naming.
- More precise relation metadata.
- Reuse existing TypeScript schema definitions.
- Easier adoption in projects that already use Drizzle.

## Provider Swap Goal

The query language should not care where metadata comes from. Swapping from
introspection metadata to Drizzle metadata should be a catalog provider change,
not a query syntax change.

Open questions:

- What metadata interface every provider must satisfy.
- How provider-specific metadata is preserved.
- How conflicts are resolved when multiple providers are combined.
- Whether generated metadata should be normalized into the existing project
  schema files.
- How to keep output stable across provider changes.

