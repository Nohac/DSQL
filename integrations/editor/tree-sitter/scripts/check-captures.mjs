import { runTreeSitter } from "./run-tree-sitter.mjs";

runTreeSitter([
  "highlight",
  "--quiet",
  "--check",
  "--captures-path",
  "queries/captures.txt",
  "test/highlight/roles.dsql",
]);
