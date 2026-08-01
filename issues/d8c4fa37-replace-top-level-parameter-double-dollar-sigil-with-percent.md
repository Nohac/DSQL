# Replace top-level parameter double-dollar sigil with percent

**ID:** d8c4fa37 | **Status:** Done | **Created:** 2026-07-31T13:50:14+02:00

Top-level parameters currently use `$$name` and anonymous `$$`. Repeated uses,
especially bounded dynamic inputs, are visually noisy and easy to confuse with
structured `$name` inputs.

Replace the top-level parameter sigil with percent:

```dsql
$name     # structured named input
$         # structured anonymous input

%name     # top-level named parameter
%         # top-level anonymous parameter

$:name    # trusted global context
```

Percent preserves a natural anonymous form, unlike alternatives such as
`$.name`, whose anonymous `$.` form looks incomplete. DSQL does not currently
have arithmetic modulo syntax; introducing it later must not reinterpret this
sigil.

This is a breaking pre-1.0 replacement. Remove `$$` completely rather than
accepting it as a compatibility alias.

Acceptance criteria:

- grammar, lowering, formatting, completion, hover, semantic tokens, and
  diagnostics recognize `%` and `%name` as top-level parameters;
- declarations, expressions, bounded operators and ordering, fragment input
  bindings, defaults, and anonymous inference use the new syntax consistently;
- generated contracts and runtime behavior remain unchanged apart from source
  spelling;
- specifications, fixtures, snapshots, and examples use the new spelling; and
- `$$` is rejected as invalid syntax with no fallback or migration bridge.

## Resolution

Top-level parameters now use `%` and `%name` throughout parsing, lowering,
formatting, completion, fragment bindings, specifications, and fixtures. The
LSP advertises `%` as a completion trigger, and completion distinguishes the
sigil token structurally so a percent wildcard inside a string is not treated
as a parameter. Directly adjacent binding sigils are invalid, preventing the
removed `$$name` spelling from being reinterpreted as two structured bindings.
