import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

export const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const cacheRoot = resolve(packageRoot, ".cache");
mkdirSync(cacheRoot, { recursive: true });

export function runTreeSitter(arguments_) {
  const result = spawnSync("tree-sitter", arguments_, {
    cwd: packageRoot,
    encoding: "utf8",
    env: { ...process.env, XDG_CACHE_HOME: cacheRoot },
  });
  process.stdout.write(result.stdout ?? "");
  process.stderr.write(result.stderr ?? "");
  if (result.status !== 0) {
    throw new Error(`tree-sitter ${arguments_.join(" ")} exited with status ${result.status}`);
  }
}
