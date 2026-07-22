# Reject invalid pagination defaults

**ID:** a6289270 | **Status:** Open | **Created:** 2026-07-22T18:52:42+02:00

Definition-default validation accepts every signed integer for `limit` and
`offset`, while planning parses their defaults as `u64`. A negative default
therefore validates successfully and then silently removes the pagination
clause, potentially turning a bounded operation into an unbounded query.

Validate pagination-role defaults as non-negative integers and never use a
failed conversion to mean that a present clause is absent.

Acceptance criteria:

- Negative and out-of-range `limit`/`offset` defaults produce targeted
  diagnostics.
- Zero and valid positive defaults continue to work.
- No validated pagination default can disappear during planning.
- Checks and SQL snapshots cover direct, lifted, and contained defaults.
