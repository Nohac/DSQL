# JSON Schema Overrides

Status: consideration.

JSON and JSONB columns often have application-level structure that is not
visible from the database catalog alone. dsql should consider allowing users to
define schema shapes for specific JSON-like columns.

## Use Case

Catalog metadata may know only this:

```text
public.events.payload jsonb
```

Project metadata could refine it:

```text
public.events.payload -> EventPayload schema
```

This would let dsql improve:

- hover information
- completion inside JSON field paths
- type checking for generated code
- runtime validation schema generation
- frontend form/table metadata

## Open Questions

- Whether overrides live in project config, schema metadata files, or separate
  schema files.
- Whether the schema format should be JSON Schema, TypeScript-derived, or a
  dsql-native shape.
- How nested JSON paths are queried in dsql.
- How provider-specific JSON operators interact with typed schema paths.
- How schema overrides are validated against real data, if at all.

