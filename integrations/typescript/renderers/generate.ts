import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  loadBuildArtifacts,
  renderDsqlHelper,
  renderTypes,
} from "@dsql/typescript/node";
import type { BuildArtifacts } from "@dsql/typescript/node";

const manifestPath = process.env.DSQL_MANIFEST;
const outDir = process.env.DSQL_OUT_DIR;

if (!manifestPath || !outDir) {
  throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
}

const artifacts = loadBuildArtifacts(manifestPath);

await renderTypes(artifacts, { outDir });
await renderDsqlHelper(artifacts, { outDir });
renderTanStackQuery({ outDir });
renderTanStackStart(artifacts, { outDir });

type RenderOptions = {
  readonly outDir: string;
};

function renderTanStackQuery(options: RenderOptions): void {
  copyTemplate("tanstack-query.ts", options.outDir);
}

function renderTanStackStart(
  artifacts: BuildArtifacts,
  options: RenderOptions,
): void {
  const template = readTemplate("tanstack-start.ts");
  const names = JSON.stringify(artifacts.operations.map((operation) => operation.name));
  writeFile(
    options.outDir,
    "tanstack-start.ts",
    `export const serverOperationNames = ${names} as const;\n${template}`,
  );
}

function copyTemplate(name: string, outDir: string): void {
  writeFile(outDir, name, readTemplate(name));
}

function readTemplate(name: string): string {
  return readFileSync(join(packageRoot(), "templates", name), "utf8");
}

function writeFile(outDir: string, name: string, contents: string): void {
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, name), contents);
}

function packageRoot(): string {
  return dirname(dirname(fileURLToPath(import.meta.url)));
}
