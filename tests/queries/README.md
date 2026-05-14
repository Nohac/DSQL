# Query Fixtures

`valid/` contains `.dsql` files expected to parse, check, format, plan, lint, and generate SQL successfully.

`invalid/` contains `.dsql` files expected to produce diagnostics. Invalid fixtures should still be useful for snapshotting diagnostics and LSP behavior.

Prefer small focused files over one very large fixture. A broad IMDb fixture set is useful, but each file should make the behavior under test obvious from its filename.

