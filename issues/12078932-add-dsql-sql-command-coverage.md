# Add dsql sql command coverage

**ID:** 12078932 | **Status:** Open | **Created:** 2026-07-17T02:14:29+02:00

The `dsql sql` subcommand has no end-to-end CLI coverage. Add an integration
test that loads a representative configured project, asserts the emitted
operation SQL and ordering, and covers its failure exit behavior without
duplicating the lower-level SQL generation tests.
