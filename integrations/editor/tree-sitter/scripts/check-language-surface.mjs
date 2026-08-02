import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import {
  KEYWORDS,
  LITERALS,
  SYMBOLS,
  TERMINAL_PATTERNS,
} from "../language-surface.mjs";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "../../..");
const grammarSource = readFileSync(
  resolve(repositoryRoot, "crates/dsql-core/src/grammar/dsql.llw"),
  "utf8",
);
const lexerSource = readFileSync(
  resolve(repositoryRoot, "crates/dsql-core/build.rs"),
  "utf8",
);
const editorGrammarSource = readFileSync(resolve(packageRoot, "grammar.js"), "utf8");

const tokenAssignments = [...grammarSource.matchAll(/\b([A-Z][A-Za-z0-9_]*)='([^']+)'/g)];
const declaredLiterals = tokenAssignments
  .map((match) => match[2])
  .filter((literal) => !(literal.startsWith("<") && literal.endsWith(">")))
  .sort();
const literalTokenNames = new Set(
  tokenAssignments
    .filter((match) => !(match[2].startsWith("<") && match[2].endsWith(">")))
    .map((match) => match[1]),
);

assertEqualSet("literal", LITERALS, declaredLiterals);

const terminalNames = [...grammarSource.matchAll(/\btoken\s+([^;]+);/g)]
  .flatMap((match) => [...match[1].matchAll(/\b([A-Z][A-Za-z0-9_]*)\b/g)])
  .map((match) => match[1])
  .filter((name) => !literalTokenNames.has(name));
assertEqualSet("terminal", Object.keys(TERMINAL_PATTERNS), terminalNames);

const lexerPatterns = new Map(
  [...lexerSource.matchAll(/\("([A-Za-z0-9_]+)",\s*r(#+)"([\s\S]*?)"\2\)/g)]
    .map((match) => [match[1], unwrapRustRawLiteral(match[3])]),
);

for (const terminal of Object.keys(TERMINAL_PATTERNS)) {
  const compilerPattern = lexerPatterns.get(terminal);
  if (compilerPattern === undefined) {
    throw new Error(`terminal ${terminal} is missing from crates/dsql-core/build.rs`);
  }
  if (compilerPattern !== TERMINAL_PATTERNS[terminal]) {
    throw new Error(
      `terminal ${terminal} pattern drifted; compiler=${JSON.stringify(compilerPattern)}, editor=${JSON.stringify(TERMINAL_PATTERNS[terminal])}`,
    );
  }
  if (!editorGrammarSource.includes(`TERMINAL_PATTERNS.${terminal}`)) {
    throw new Error(`terminal ${terminal} is declared but unused by grammar.js`);
  }
}

assertInventoryUsed("keyword", "K", KEYWORDS, editorGrammarSource);
assertInventoryUsed("symbol", "S", SYMBOLS, editorGrammarSource);

console.log(
  `language surface matches ${declaredLiterals.length} literals and ${terminalNames.length} terminals`,
);

function assertEqualSet(label, actual, expected) {
  const actualSet = new Set(actual);
  const expectedSet = new Set(expected);
  const missing = [...expectedSet].filter((item) => !actualSet.has(item));
  const extra = [...actualSet].filter((item) => !expectedSet.has(item));
  if (missing.length > 0 || extra.length > 0) {
    throw new Error(
      `${label} surface drifted; missing=[${missing.join(", ")}], extra=[${extra.join(", ")}]`,
    );
  }
}

function assertInventoryUsed(label, namespace, inventory, source) {
  const unused = Object.keys(inventory).filter((name) => !source.includes(`${namespace}.${name}`));
  if (unused.length > 0) {
    throw new Error(`${label} inventory entries are unused: ${unused.join(", ")}`);
  }
}

function unwrapRustRawLiteral(source) {
  if (!source.startsWith("r")) {
    throw new Error(`expected nested Rust raw literal, found ${source}`);
  }
  const openingQuote = source.indexOf('"');
  const hashes = source.slice(1, openingQuote);
  const closingDelimiter = `"${hashes}`;
  const closingQuote = source.indexOf(closingDelimiter, openingQuote + 1);
  if (openingQuote < 1 || closingQuote < 0) {
    throw new Error(`invalid nested Rust raw literal ${source}`);
  }
  return source.slice(openingQuote + 1, closingQuote);
}
