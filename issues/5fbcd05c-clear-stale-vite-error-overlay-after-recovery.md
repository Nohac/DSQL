# Clear stale Vite error overlay after recovery

**ID:** 5fbcd05c | **Status:** Done | **Created:** 2026-07-19T22:45:47+02:00

After a compile error, the Vite integration retained its browser error overlay
after dsql compiled successfully again. Recovery only produced a browser event
when full reloads were enabled, and a successful `changed: false` replay left
the stored error active for later browser connections.

Successful rendered generations now explicitly clear the overlay. No-op
replays do the same only when their generation matches the active rendered
state, so a renderer failure cannot be hidden by a newer daemon no-op.
