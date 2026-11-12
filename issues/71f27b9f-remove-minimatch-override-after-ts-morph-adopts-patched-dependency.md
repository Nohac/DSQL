# Remove minimatch override after ts-morph adopts patched dependency

**ID:** 71f27b9f | **Status:** Open | **Created:** 2026-11-12T13:19:18+01:00

`@ts-morph/common` 0.29.0 requires minimatch 10, which is affected by
[GHSA-g3hv-75wg-m249](https://github.com/advisories/GHSA-g3hv-75wg-m249).
The TypeScript integration overrides minimatch to 11.0.4, which also selects
patched brace-expansion 5.0.12. This respects the seven-day release-age policy.
Type checking and all 108 integration tests pass with the override; Bun audit is clean.

Remove the override when ts-morph's dependency chain selects patched versions
without it. Re-run Bun audit, type checking, and tests with the real daemon.
