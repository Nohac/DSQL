import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

import { packageRoot, runTreeSitter } from "./run-tree-sitter.mjs";

const openingDelimiters = new Set(["{", "(", "["]);
const grammar = JSON.parse(readFileSync(resolve(packageRoot, "src/grammar.json"), "utf8"));
const query = readFileSync(resolve(packageRoot, "queries/indents.scm"), "utf8");

function containsOpeningDelimiter(node) {
  if (Array.isArray(node)) {
    return node.some(containsOpeningDelimiter);
  }
  if (node === null || typeof node !== "object") {
    return false;
  }
  if (node.type === "STRING" && openingDelimiters.has(node.value)) {
    return true;
  }
  return Object.values(node).some(containsOpeningDelimiter);
}

function stripQueryComments(source) {
  let result = "";
  let inString = false;
  let escaped = false;
  let inComment = false;

  for (const character of source) {
    if (inComment) {
      if (character === "\n") {
        inComment = false;
        result += character;
      }
      continue;
    }
    if (inString) {
      result += character;
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === "\"") {
        inString = false;
      }
      continue;
    }
    if (character === ";") {
      inComment = true;
    } else {
      result += character;
      if (character === "\"") {
        inString = true;
      }
    }
  }
  return result;
}

const expectedContainers = Object.entries(grammar.rules)
  .filter(([name, rule]) => !name.startsWith("_") && containsOpeningDelimiter(rule))
  .map(([name]) => name)
  .sort();

const queryWithoutComments = stripQueryComments(query);
const beginBlocks = [...queryWithoutComments.matchAll(/\[([\s\S]*?)\]\s*@indent\.begin/g)];
if (beginBlocks.length !== 2) {
  throw new Error(`expected structural and continuation @indent.begin blocks, found ${beginBlocks.length}`);
}
const actualContainers = [...beginBlocks[0][1].matchAll(/\(([a-z_]+)\)/g)]
  .map(match => match[1])
  .sort();

if (JSON.stringify(actualContainers) !== JSON.stringify(expectedContainers)) {
  throw new Error([
    "indent container coverage differs from src/grammar.json",
    `expected: ${expectedContainers.join(", ")}`,
    `actual:   ${actualContainers.join(", ")}`,
  ].join("\n"));
}

const expectedContinuations = ["order_by_clause", "pipe_transform"];
const actualContinuations = [...beginBlocks[1][1].matchAll(/\(([a-z_]+)\)/g)]
  .map(match => match[1])
  .sort();
if (JSON.stringify(actualContinuations) !== JSON.stringify(expectedContinuations)) {
  throw new Error([
    "indent continuation coverage changed",
    `expected: ${expectedContinuations.join(", ")}`,
    `actual:   ${actualContinuations.join(", ")}`,
  ].join("\n"));
}

const captures = new Set(
  [...queryWithoutComments.matchAll(/@([a-z.]+)/g)].map(match => match[1]),
);
const allowedCaptures = new Set(["indent.begin", "indent.branch", "indent.end"]);
for (const capture of captures) {
  if (!allowedCaptures.has(capture)) {
    throw new Error(`unsupported indentation capture @${capture}`);
  }
}
for (const capture of allowedCaptures) {
  if (!captures.has(capture)) {
    throw new Error(`missing indentation capture @${capture}`);
  }
}

const parserPath = resolve(packageRoot, ".cache/dsql.so");
runTreeSitter(["build", "--output", parserPath]);

const result = spawnSync("nvim", [
  "--headless",
  "-u", "NONE",
  "-n",
  "-l", "scripts/check-indents.lua",
], {
  cwd: packageRoot,
  encoding: "utf8",
  env: {
    ...process.env,
    DSQL_TREE_SITTER_ROOT: packageRoot,
    DSQL_TREE_SITTER_PARSER: parserPath,
    NVIM_LOG_FILE: resolve(packageRoot, ".cache/nvim.log"),
    XDG_STATE_HOME: resolve(packageRoot, ".cache/nvim-state"),
  },
});

process.stdout.write(result.stdout ?? "");
process.stderr.write(result.stderr ?? "");
if (result.error) {
  throw new Error(`failed to start Neovim: ${result.error.message}`);
}
if (result.status !== 0) {
  throw new Error([
    `Neovim indentation oracle exited with status ${result.status}`,
    "Install nvim-treesitter or set DSQL_NVIM_TREESITTER_PATH to its checkout.",
  ].join("\n"));
}

console.log(
  `indentation: ${actualContainers.length} delimiter containers and ${actualContinuations.length} continuations covered; Neovim oracle passed`,
);
