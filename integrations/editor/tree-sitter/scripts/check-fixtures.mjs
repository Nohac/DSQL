import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "../../..");
const fixturePaths = collectDsqlFiles(repositoryRoot).filter((path) => {
  const relativePath = relative(repositoryRoot, path);
  return !relativePath.startsWith("target/")
    && !relativePath.startsWith("integrations/editor/tree-sitter/test/");
});

if (fixturePaths.length === 0) {
  throw new Error("no DSQL fixtures were found");
}

const temporaryRoot = mkdtempSync(join(tmpdir(), "dsql-tree-sitter-"));
const pathList = join(temporaryRoot, "fixtures.txt");
writeFileSync(pathList, fixturePaths.join("\n"));

try {
  const result = spawnSync(
    "tree-sitter",
    ["parse", "--grammar-path", packageRoot, "--paths", pathList, "--json-summary"],
    {
      cwd: repositoryRoot,
      encoding: "utf8",
      env: { ...process.env, XDG_CACHE_HOME: join(temporaryRoot, "cache") },
    },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    process.stdout.write(result.stdout);
    throw new Error(`Tree-sitter fixture parse exited with status ${result.status}`);
  }

  const jsonStart = result.stdout.indexOf("{");
  if (jsonStart < 0) {
    throw new Error(`Tree-sitter produced no JSON summary:\n${result.stdout}`);
  }
  const summary = JSON.parse(result.stdout.slice(jsonStart));
  const failures = summary.parse_summaries.filter((item) => !item.successful);
  if (failures.length > 0) {
    throw new Error(
      `Tree-sitter failed to parse:\n${failures.map((item) => item.file).join("\n")}`,
    );
  }

  console.log(`parsed ${fixturePaths.length} DSQL fixtures without errors or missing nodes`);
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true });
}

function collectDsqlFiles(root) {
  const paths = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (entry.name === ".git" || entry.name === "node_modules" || entry.name === "target") {
      continue;
    }
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      paths.push(...collectDsqlFiles(path));
    } else if (entry.isFile() && entry.name.endsWith(".dsql")) {
      // Force an eager read so permission errors are reported by this guard.
      readFileSync(path);
      paths.push(path);
    }
  }
  return paths.sort();
}
