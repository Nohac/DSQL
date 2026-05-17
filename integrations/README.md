# DSQL Integrations

This directory contains experimental host-language and framework integrations.

Integrations should consume DSQL build metadata instead of reimplementing parser,
checker, planner, or SQL generation behavior. The stable boundary is intended to
be checked SQL plus JSON metadata emitted by the Rust tooling.

Current integrations:

- [`typescript`](typescript/) - experimental TypeScript metadata/runtime helpers.
