# Only surface generation errors in the Vite plugin

**ID:** 45c154fa | **Status:** Done | **Created:** 2026-07-17T17:04:21+02:00

The Vite integration currently reports every compiler diagnostic, including
warnings. This makes routine development noisy even though warnings do not
prevent artifact generation or make the generated module unusable.

Only error-severity diagnostics that block or invalidate generation should be
surfaced through the Vite plugin. Warnings should remain available through the
explicit project validation workflow:

```sh
dsql validate
```

Preserve the daemon protocol full-diagnostic snapshot and CLI behavior; filter
at the Vite consumer boundary where diagnostics become development-server
errors. Add integration coverage showing that warning-only compilation remains
quiet and usable while an error still reaches the Vite error path. Mixed failed
responses must render only the blocking errors, not the full warning snapshot.

The original consumer-side filtering was incomplete as a contract: it left
every binding responsible for understanding diagnostic severity and allowed
stale built package output to restore warning noise. Diagnostic visibility
must instead be configured when the daemon session initializes, with Vite
requesting errors only and rendering the returned snapshot unchanged.

Resolved by adding the daemon protocol's `diagnosticLevel` session setting.
The daemon filters both success and failure snapshots after its full internal
error gate; Vite requests `error` and performs no severity filtering itself.
