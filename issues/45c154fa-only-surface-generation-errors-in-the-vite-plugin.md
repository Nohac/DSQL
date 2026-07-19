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

Resolved by filtering failed daemon responses at the Vite rendering boundary.
The mixed-diagnostic integration case verifies that the blocking error remains
visible while warning codes and messages are omitted.
