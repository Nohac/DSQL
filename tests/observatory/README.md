# Observatory conformance database

This project is the deterministic PostgreSQL target for live correctness tests
and future benchmarks. It covers composite keys, every supported scalar type,
catalog comments, nested relations, filters, aggregates, runtime sort variants,
and resolution scopes. Shared filters and fragments are imported by separate
`api` and `analytics` operation scopes.

## Coverage

The project keeps stable executable features tied to named operations so a
missing conformance case is visible during review.

| Feature area | Conformance sources |
| --- | --- |
| Catalog types, composite keys, comments, views, and materialized views | `schema.sql`, generated `dsql/schema/`, and the live introspection test |
| Resolution scopes, terminal targets, and transitive shared definitions | `dsql.toml`, `project.generated.ts`, `shared`, `api`, and `analytics` |
| Structured inputs, top-level params, and trusted context | `TypedReading`, `RecentReadings`, and `TenantScope` |
| Scalar, collection, enum, boolean, and null defaults | `RecentReadings`, `ManualFilterProbe`, and `SensorReadingWindow` |
| Nullable predicate pruning, reversed operands, and optional pagination | `OptionalPredicateProbe`, `SensorReadingWindow`, and `MappedSensorWindow` |
| Bounded dynamic predicates and ordering over selected fields | `DynamicReadingSearch` |
| Fragment containment and whole-root lifting | `ContainedSensorWindow` and `LiftedSensorWindow` |
| Namespaced and cross-root leaf bindings, including forwarding shorthand | `NamespacedSensorWindow` and `MappedSensorWindow` |
| Nested composite-key relations, existence, ordering, and pagination | `NetworkTopology` and `SensorReadingWindow` |
| Equality, range, membership, null, boolean, and aggregate predicates | `TypedReading`, `PrivacyProbe`, and the `EmptyAggregate*` operations |
| Row filters, field masking, and conditional manual filters | `TenantScope`, `ReadingPrivacy`, `FlaggedOnly`, and `ManualFilterProbe` |
| Scalar, grouped, nested, and flattened aggregates | the `analytics` scope |
| Singular inference and root flattening | `TypedReading` and `MissingFlattened` |
| Native generation, materialization, execution, and scope listing | the `dsql-cli` Observatory integration tests |

Intentionally unimplemented language features are not represented as fake
coverage. Directive execution, split fetch, and mutations receive Observatory
cases when their executable contracts land. Editor-only behavior remains in the
core and LSP protocol suites.

The Bun lifecycle talks to PostgreSQL directly through `SQL`; only container
lifecycle operations shell out, and those use rootless Podman. PostgreSQL is
published on an automatically allocated `127.0.0.1` port.

```sh
bun run start
bun run url
bun run reset --profile=small
bun run stop
```

Profiles are `correctness` (the default), `small`, `medium`, and `large`. Seed
rows are formula-derived from a fixed timestamp, so repeated resets are stable.
Starting with a different profile replaces and reseeds an already-running
container. The ignored `dsql/build/observatory.json` is mode `0600` because it
contains the generated password for the ephemeral database. `start` and
`reset` also write the URL to an ignored project-root `.env`; `stop` removes
both files. The `.env` is the CLI's conventional connection interface, while
the container bookkeeping remains disposable build state.

The dsql CLI reads `DSQL_DATABASE_URL` from that project-root `.env`, so
database-touching commands work directly after `start`:

```sh
dsql introspect
dsql operation execute NetworkTopology \
  --scope api \
  --context '{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a301"}'
```

An explicitly exported `DSQL_DATABASE_URL` takes precedence over `.env`, and
the tracked `database_url` is the fallback when neither supplies a value.

Live Rust tests remain opt-in and hermetic by default:

```sh
DSQL_OBSERVATORY_DATABASE_URL=<url> cargo test --workspace
```

Set `DSQL_POSTGRES_IMAGE` to test another PostgreSQL image. The default is a
pinned PostgreSQL minor release rather than whichever image happens to be
installed locally.
