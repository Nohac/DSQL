# Share fragment spread expansion across analysis passes

**ID:** 472b2936 | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Fragment spread expansion should be centralized and reused by checking, linting,
planning, and generation instead of each pass handling spreads differently.

## Context

Current behavior is inconsistent:

- Checking validates spread existence and `on` target compatibility, but does
  not recurse through spread selections in the use context after validation.
- Planning expands fragment selections recursively, but the recursive planner
  path does not carry an explicit visiting set for cycle diagnostics.
- Linting currently skips fragment spread selections in query-definition linting
  and primarily reports diagnostics at fragment definition sites.
- Generation has its own recursive fragment-spread metadata traversal with cycle
  protection.

The shared helper should keep deterministic traversal order, preserve source
ranges, and distinguish fragment-owned problems from use-site problems.

## Done When

- A shared fragment expansion/check helper is used by the relevant analysis
  passes.
- Cyclic spreads produce deterministic diagnostics and cannot recurse
  indefinitely.
- Use-context checks and lints can inspect spread-expanded selections against
  the actual parent relation/table context.
- Existing cross-file fragment behavior continues to work.
- Focused regression tests cover direct cycles, transitive cycles, and
  use-context fragment diagnostics.
