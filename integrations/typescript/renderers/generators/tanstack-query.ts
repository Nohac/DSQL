import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { BuildArtifacts, DsqlRenderResult } from "@dsql/typescript/node";
import {
  Project,
  QuoteKind,
  VariableDeclarationKind,
} from "@dsql/typescript/renderer";

type RenderOptions = {
  readonly outDir: string;
  readonly root?: string;
};

export async function renderTanStackQuery(
  artifacts: BuildArtifacts,
  dsql: DsqlRenderResult,
  options: RenderOptions,
): Promise<void> {
  const root = resolve(options.root ?? process.cwd());
  const sourcePath = join(options.outDir, "tanstack-query.ts");
  const project = createProject();
  const source = createSourceFromTemplate(
    project,
    options.outDir,
    "tanstack-query.ts",
  );

  if (artifacts.operations.length > 0) {
    source.addImportDeclaration({
      moduleSpecifier: importSpecifier(root, sourcePath, dsql.modules.queries),
      namedImports: artifacts.operations.map(
        (operation) => `${toPascalCase(operation.name)}Operation`,
      ),
    });
    source.addImportDeclaration({
      moduleSpecifier: "./tanstack-start",
      namedImports: artifacts.operations.map(
        (operation) => `${toPascalCase(operation.name)}ServerFn`,
      ),
    });
  }

  source.addVariableStatement({
    declarationKind: VariableDeclarationKind.Const,
    declarations: [
      {
        name: "serverFunctions",
        type: "Record<string, DsqlServerFunction<any>>",
        initializer: `{
${artifacts.operations
  .map((operation) => {
    const name = toPascalCase(operation.name);
    return `  ${JSON.stringify(operation.name)}: ${name}ServerFn as DsqlServerFunction<typeof ${name}Operation>`;
  })
  .join(",\n")}
}`,
      },
    ],
  });

  source.formatText();
  await source.save();
}

function createProject(): Project {
  return new Project({
    manipulationSettings: {
      quoteKind: QuoteKind.Double,
    },
  });
}

function createSourceFromTemplate(
  project: Project,
  outDir: string,
  name: string,
) {
  mkdirSync(outDir, { recursive: true });
  return project.createSourceFile(
    join(outDir, name),
    templateContents(name),
    { overwrite: true },
  );
}

function templateContents(name: string): string {
  return readFileSync(templatePath(name), "utf8").replace(
    /^\/\/ @ts-nocheck\r?\n/,
    "",
  );
}

function templatePath(name: string): string {
  const generatorDir = dirname(fileURLToPath(import.meta.url));
  const candidates = [
    join(generatorDir, "templates", name),
    join(dirname(generatorDir), "templates", name),
  ];
  const path = candidates.find((candidate) => existsSync(candidate));
  if (!path) {
    throw new Error(`missing TanStack template ${name}`);
  }
  return path;
}

function toPascalCase(value: string): string {
  const result = value
    .split(/[^A-Za-z0-9]+/)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join("");

  if (!result) {
    return "Operation";
  }

  return /^[0-9]/.test(result) ? `_${result}` : result;
}

function importSpecifier(root: string, fromFile: string, modulePath: string): string {
  if (!modulePath.startsWith(".")) {
    return modulePath;
  }

  const absoluteModulePath = resolve(root, modulePath);
  const relativePath = relative(dirname(fromFile), absoluteModulePath)
    .split("\\")
    .join("/");
  return relativePath.startsWith(".") ? relativePath : `./${relativePath}`;
}
