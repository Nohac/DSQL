# Runtime collection limits are dropped when the default cap applies

**ID:** 2f03cb9b | **Status:** Open | **Created:** 2026-07-18T13:39:43+02:00

When a collection uses a runtime `limit` and SQL generation also applies the
configured default collection cap, the cap replaces the runtime parameter
instead of bounding it. For example, `title(limit $$count)` renders `limit 10`
under the default cap and omits `params.count` from the SQL parameter list.

Preserve the runtime limit while enforcing the cap, with SQL and metadata tests
covering values below, equal to, and above the cap.
