# Metadata Sources

Status: consideration.

dsql should support multiple metadata sources instead of assuming PostgreSQL
introspection is the only way to build the catalog.

Providers produce the generated catalog described by
[Catalog Metadata](catalog-metadata.md). Project configuration may direct a
provider to capture additional database facts, while authored overlays modify
the generated facts into one effective catalog. Compiler stages consume only
that effective catalog; provider and overlay composition is completed before
language resolution begins.

## Sources To Consider

- PostgreSQL introspection.
- Hasura metadata.
- Hardcoded or generated project metadata.
- Drizzle schema metadata.
- ORM metadata from other ecosystems.
- Hand-authored schema files.

## Config-Directed Introspection

Some database semantics cannot be discovered from structural metadata alone.
For example, an ordinary table does not declare whether its rows are a closed,
migration-managed value set. Narrow project configuration may tell an
introspection provider which additional database facts to capture without
embedding arbitrary SQL or transformation rules in dsql configuration.

[Enumerated Types](enums.md) uses this model for table- and view-backed enums:
`dsql/dsql.toml` identifies the source and structural columns, introspection
captures their values in the generated catalog, and the effective catalog
normalizes them with provider-native enums. A database view is the escape hatch
when the captured source requires filtering or transformation.

## Drizzle

Drizzle can already describe tables, columns, relations, and application-level
types. A Drizzle provider could let dsql reuse that metadata rather than
requiring database introspection.

Potential benefits:

- Better application-level naming.
- More precise relation metadata.
- Reuse existing TypeScript schema definitions.
- Easier adoption in projects that already use Drizzle.

## Hasura

Hasura metadata is a useful source for named relationships that may not exist as
database foreign keys.

Potential imported metadata:

- Object relationships.
- Array relationships.
- Relationship names.
- Source and target tables or views.
- Column mappings and manual join definitions.

This could let projects migrate existing Hasura relationship names into dsql
defined relationship aliases instead of exposing only raw relation edge
selectors such as `users->assignee_id`.

## Provider Swap Goal

The query language should not care where metadata comes from. Swapping from
introspection metadata to Drizzle metadata should be a catalog provider change,
not a query syntax change.

Open questions:

- What metadata interface every provider must satisfy.
- How provider-specific metadata is preserved.
- How conflicts are resolved when multiple providers are combined.
- Which provider enrichments are serialized into the existing generated schema
  files and how their configuration fingerprints are checked.
- How to keep output stable across provider changes.
