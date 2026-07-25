# Operation Execution

Status: implemented for PostgreSQL query operations.

Execution consumes the same assembled operation metadata that generators and
the build daemon publish. It never parses source independently or re-resolves
catalog fields.

## CLI

```text
dsql operation list [--scope <scope>]
dsql operation execute <name> --scope <scope> \
  [--variables <json> | --variables-file <path>] \
  [--context <json> | --context-file <path>]
```

`op` is a visible alias for `operation`. Listing compiles the whole project and
prints the scope and name of each clean operation without connecting to the
database. Execution also compiles the whole project and refuses to run while
error diagnostics exist.

Variables preserve the metadata namespaces:

```json
{
  "params": { "ids": [1, 2], "direction": "desc" },
  "input": { "readings": { "clause": { "limit": 10 } } }
}
```

Trusted context is supplied separately, without a `context` wrapper:

```json
{ "tenant_id": "018f6f19-795f-7c3d-b1b3-8f177ab8a301" }
```

This separation is a trust boundary. Context values assert server-owned
identity or authorization facts; public request data must not be copied into
context without validation.

The configured `database_url` is used by default. Database-touching CLI
commands read `DSQL_DATABASE_URL` from the process environment first, then from
`.env` beside the project root's `dsql/` directory. This lets ephemeral
databases use dynamic ports without rewriting tracked configuration. Exported
process values always take precedence; a missing `.env` or one without that key
falls back to `dsql.toml`, while a malformed or unreadable `.env` is an error
when the file is consulted.

## Materialization

Before execution, every positional parameter path is joined to its one
authoritative `InputField` declaration. Missing paths, invalid logical values,
and unsupported types fail before database execution. SQL variants are replaced
only by text from their compiler-produced closed case list; caller text is
never interpolated into SQL.

Materialization first substitutes typed declaration defaults for omitted public
paths. It then checks requiredness and validates values. A supplied value always
wins over a default; explicit `null` is accepted only for a nullable declaration.
Trusted context is never defaulted by query source.

Nullable values with structural roles are materialized through compiler-owned
semantic cases, not by interpolating or rewriting caller text. In particular, a
null predicate operand selects the case where its complete predicate atom is
absent, and null pagination or dynamic input selects the corresponding absent
clause or identity case. Execution must preserve the pruning rules defined in
[Nullable Predicate Uses](variables.md#nullable-predicate-uses).

The executor binds PostgreSQL values according to the declared logical type and
collection shape. Every query definition produces one generated statement that
returns exactly one result row. A multi-root definition renders its collection,
singular, aggregate, and flattened roots as one-row subqueries and combines
them in source order; all roots share one parameter and variant namespace. The
executor serializes the complete generated row as one JSON value, so
single-root, multi-root, and multi-column flattened operations all obey the
same metadata result shape.

`PostgresExecutor::connect` creates a single-connection pool for one-shot CLI
use. Long-lived library callers can provide their own pool through
`PostgresExecutor::from_pool`.

JSON input encodings are:

| Logical type | JSON encoding |
| --- | --- |
| `uuid` | UUID string |
| `text` | string |
| `timestamptz` | RFC 3339 string |
| `int` | integer number between `-9007199254740991` and `9007199254740991` |
| `numeric` | decimal string, preserving arbitrary precision |
| `float` | number |
| `boolean` | boolean |
| `json` | any JSON value |

A collection uses an array of the corresponding scalar encoding. Supplied
collections may contain `null` elements; declaration collection defaults may
not. The `int` range is the exact-integer domain of IEEE-754 doubles, which JSON
numbers take when parsed by JavaScript. Both supplied values and declaration
defaults outside it are rejected before database execution. Use `numeric` when
an exact value outside that range is required.

Input refinements determine requiredness, nullability, and defaults in emitted
metadata. The maintained Rust and TypeScript materializers consume that same
contract; adapters must not reinterpret it independently.

## Test database

`tests/observatory` is the deterministic live conformance and benchmark target.
Its Bun lifecycle starts rootless Podman on an automatically selected loopback
port, uses Bun `SQL` for readiness/schema/seeding, and leaves ordinary
`cargo test` hermetic when no observatory URL is supplied.
