# DSQL TypeScript Integration

Experimental TypeScript integration for DSQL build metadata.

This package should stay metadata-first:

1. The Rust CLI/compiler emits checked SQL and JSON metadata under
   `dsql/build/manifest.json`.
2. `dsql generate` runs configured commands that write owned application code
   from the manifest.
3. This package provides convenience types and helpers for tools that interact
   with those build artifacts.

The first target is a generator command that reads the manifest path from
`DSQL_MANIFEST`, loads templates, and writes owned files into `DSQL_OUT_DIR`.
It is intentionally a normal Bun/TypeScript program so projects can vendor or
replace it.

```toml
[generate.typescript]
enabled = true
out_dir = "src/generated/dsql"
cmd = ["bun", "node_modules/@dsql/typescript/src/generator.ts"]
```

The exact runtime API is still open. This package is intentionally small until
the DSQL build metadata format stabilizes.

## Metadata Types

The metadata contract is owned by Rust in the `dsql-metadata` crate. The CLI can
generate the TypeScript integration artifacts from that contract:

```sh
cargo run -p dsql-cli -- generate --target typescript-metadata --out-dir integrations/typescript/src/generated
```

This package keeps generated schema/types under `src/generated`. TypeScript
types are generated directly from the Rust `Facet` metadata via
`facet-typescript`; the JSON Schema is kept as an additional integration
artifact.

```sh
bun run generate
```

The lower-level `metadata-schema` and `metadata-typescript` commands remain
available for debugging or piping metadata into other tools.

The checked-in generated files are bootstrap artifacts while the metadata format
is still moving.
