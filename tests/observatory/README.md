# Observatory conformance database

This project is the deterministic PostgreSQL target for live correctness tests
and future benchmarks. It covers composite keys, every supported scalar type,
catalog comments, nested relations, filters, aggregates, and dynamic inputs.

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
container. The ignored `.db-state.json` is mode `0600` because it contains the
generated password for the ephemeral database.

Database-touching dsql commands accept the printed URL through the environment:

```sh
DSQL_DATABASE_URL=<url> dsql introspect
DSQL_DATABASE_URL=<url> dsql operation execute NetworkTopology \
  --scope default \
  --context '{"tenant_id":"018f6f19-795f-7c3d-b1b3-8f177ab8a301"}'
```

Live Rust tests remain opt-in and hermetic by default:

```sh
DSQL_OBSERVATORY_DATABASE_URL=<url> cargo test --workspace
```

Set `DSQL_POSTGRES_IMAGE` to test another PostgreSQL image. The default is a
pinned PostgreSQL minor release rather than whichever image happens to be
installed locally.
