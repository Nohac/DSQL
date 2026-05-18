import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  loadBuildArtifacts,
  renderDsqlHelper,
  renderTypes,
} from "@dsql/typescript/node";
import type { BuildArtifacts } from "@dsql/typescript/node";
import {
  tanstackQueryTemplate,
  tanstackStartTemplate,
} from "./templates/my-templates";

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
  writeFile(options.outDir, "tanstack-query.ts", tanstackQueryTemplate);
}

function renderTanStackStart(
  artifacts: BuildArtifacts,
  options: RenderOptions,
): void {
  writeFile(
    options.outDir,
    "tanstack-start.ts",
    tanstackStartTemplate(
      artifacts.operations.map((operation) => operation.name),
    ),
  );
}

function writeFile(outDir: string, name: string, contents: string): void {
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, name), contents);
}
