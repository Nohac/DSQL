import { mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Project, QuoteKind } from "ts-morph";
import type { BuildArtifacts } from "../node";
import type { RenderOptions } from "./types";

export type OperationPredicate = (
  operation: BuildArtifacts["operations"][number],
) => boolean;

export type TanStackRenderOptions = RenderOptions & {
  readonly only?: OperationPredicate;
};

export async function renderTanStackQuery(
  artifacts: BuildArtifacts,
  options: TanStackRenderOptions,
): Promise<void> {
  await copyTemplate("tanstack-query.ts", options.outDir);
}

export async function renderTanStackStart(
  artifacts: BuildArtifacts,
  options: TanStackRenderOptions,
): Promise<void> {
  const operations = options.only
    ? artifacts.operations.filter(options.only)
    : artifacts.operations;
  const project = new Project({
    manipulationSettings: {
      quoteKind: QuoteKind.Double,
    },
  });
  const source = project.createSourceFile(
    join(options.outDir, "tanstack-start.ts"),
    readFileSync(join(packageRoot(), "templates", "tanstack-start.ts"), "utf8"),
    { overwrite: true },
  );
  source.insertStatements(
    0,
    `export const serverOperationNames = ${JSON.stringify(
      operations.map((operation) => operation.name),
    )} as const;`,
  );
  source.formatText();
  await source.save();
}

async function copyTemplate(name: string, outDir: string): Promise<void> {
  mkdirSync(outDir, { recursive: true });
  const project = new Project({
    manipulationSettings: {
      quoteKind: QuoteKind.Double,
    },
  });
  const source = project.createSourceFile(
    join(outDir, name),
    readFileSync(join(packageRoot(), "templates", name), "utf8"),
    { overwrite: true },
  );
  source.formatText();
  await source.save();
}

function packageRoot(): string {
  return dirname(dirname(dirname(fileURLToPath(import.meta.url))));
}
