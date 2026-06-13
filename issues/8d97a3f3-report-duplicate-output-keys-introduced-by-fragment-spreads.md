# Report duplicate output keys introduced by fragment spreads

**ID:** 8d97a3f3 | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Duplicate output-key detection should include fields introduced by fragment
spreads, not only fields written directly at the selection site.

## Context

`check_duplicate_output_keys` skips `SelectionKind::FragmentSpread`, so a query
can become invalid only after a spread is expanded without being reported.

Example:

```dsql
fragment MovieInfoFields on movie_info {
  note
}

query MovieInfo {
  movie_info {
    note
    ...MovieInfoFields
  }
}
```

The local `note` selection and the fragment `note` selection produce the same
public response key. Aliases and qualified relation names should keep using the
existing response-key rules.

## Done When

- Duplicate output keys are checked after fragment expansion.
- Diagnostics point at a useful range, preferably the duplicate field or spread
  use site depending on where the duplicate is introduced.
- Aliased fields continue to disambiguate output keys.
- Focused tests cover local-vs-fragment and fragment-vs-fragment duplicates.
