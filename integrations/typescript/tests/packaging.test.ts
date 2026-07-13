// The built dist must load under plain Node ESM — Bun resolves
// extensionless specifiers that Node rejects, so only Node proves the
// emitted import specifiers are complete.
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { expect, test } from "bun:test";

const PACKAGE_DIR = join(import.meta.dir, "..");

function nodeAvailable(): boolean {
  try {
    execFileSync("node", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

const packaging = nodeAvailable() ? test : test.skip;

packaging("every built entry loads under Node ESM", () => {
  execFileSync("bunx", ["tsc"], { cwd: PACKAGE_DIR, stdio: "ignore" });
  for (const entry of ["index.js", "runtime.js", "node.js", "renderer.js", "vite.js"]) {
    const target = join(PACKAGE_DIR, "dist", entry);
    // A failed import exits nonzero and execFileSync throws — the exit
    // code is the assertion (sandboxes may not pass nested stdout
    // through reliably).
    expect(() =>
      execFileSync(
        "node",
        ["--input-type=module", "-e", `await import(${JSON.stringify(target)});`],
        { cwd: PACKAGE_DIR, stdio: "ignore" },
      ),
    ).not.toThrow();
  }
}, 120_000);
