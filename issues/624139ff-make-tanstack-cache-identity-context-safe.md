# Make TanStack cache identity context-safe

**ID:** 624139ff | **Status:** Done | **Created:** 2026-07-18T22:47:07+02:00

The generated TanStack Query adapter keys results by operation and public
variables. Operations that depend on trusted server context can therefore reuse
stale results if authentication changes while the same `QueryClient` remains
alive.

Add an application-supplied opaque context-scope discriminator to cache keys
without exposing trusted context values to the client. Generated query helpers
now require `contextScope` for context-dependent operations and include it in
the cache key without sending it to the server function.
