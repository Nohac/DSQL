# Complete aggregate pipe keywords and group keys

**ID:** 1ac8278a | **Status:** Done | **Created:** 2026-07-17T16:40:56+02:00

Aggregate pipe completion has two contextual gaps:

```dsql
field(where ...) | aggr<cursor> { count }
field | aggregate by <cursor> { count }
```

After `|`, partial keyword completion should offer `aggregate` and replace the
partial word. After `aggregate by`, group-key completion should insert a valid
rooted operand such as `.production_year`; it currently offers bare field names
that produce invalid syntax. Completion after an already typed `.` works.

Add integration snapshots covering the empty and partial pipe keyword,
replacement ranges, group-key completion with and without a leading `.`, and
the resulting insert text.
