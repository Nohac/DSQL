# Declare project type mappings in dsql.toml

**ID:** fe7d14cf | **Status:** Done | **Created:** 2026-07-26T11:48:49+02:00

Projects cannot name a PostgreSQL type the compiler does not already know. A
`date` or `citext` column is reported as `unknown` in generated metadata, which
erases the information a generator would need to give it a host type. There is no
authored surface between "the compiler hardcodes this type" and "this type does
not exist".

Add a structural type declaration to `dsql.toml`, beside the `[[catalog.enums]]`
surface specified in docs/spec/enums.md:

```toml
[[catalog.types]]
pg = "pg_catalog.date"
name = "Date"
wire = "text"
literal = "string"
pattern = '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'
```

`name` is the nominal logical type that appears in generated metadata, `wire`
selects a declared encoding class (1c36c801), `literal` states which dsql literal
kind may compare against the type, and `pattern` optionally retains
pre-execution validation. Operators default to the introspected set and may only
be narrowed.

Configuration stays structural. It cannot contain SQL, expressions, or
transforms, matching the constraint enums.md places on enum configuration.

Sequenced after 1c36c801. The host-language half is 920e30bf.

Acceptance criteria:

- A declared type is reported by its nominal name in operation metadata rather
  than as `unknown`.
- Declaring a type the provider does not have, or narrowing to an operator the
  provider does not support, is a project-load error naming the file and key.
- A declared `pattern` rejects a bad input before execution with the same error
  shape as built-in scalars.
- Manifest consumers see the new `data_type` values behind a manifest version
  bump; the version story is stated in docs/spec/codegen.md.
- docs/spec/catalog-metadata.md documents the declaration and its interaction
  with the generated `type_map.yaml`.

Implemented with strict `[[catalog.types]]` validation, effective domain and
array propagation, manifest v6 input-validation metadata, and shared Rust and
TypeScript runtime pattern conformance. Older manifest contracts are rejected.
