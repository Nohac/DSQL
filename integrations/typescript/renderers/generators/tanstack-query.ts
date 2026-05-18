import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { BuildArtifacts } from "@dsql/typescript/node";
import {
  Project,
  QuoteKind,
  VariableDeclarationKind,
} from "@dsql/typescript/renderer";

type RenderOptions = {
  readonly outDir: string;
};

export async function renderTanStackQuery(
  artifacts: BuildArtifacts,
  options: RenderOptions,
): Promise<void> {
  const project = createProject();
  const source = createSourceFromTemplate(
    project,
    options.outDir,
    "tanstack-query.ts",
  );

  if (artifacts.operations.length > 0) {
    source.addImportDeclaration({
      moduleSpecifier: "./operations",
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

  const queriesSource = project.createSourceFile(
    join(options.outDir, "queries.ts"),
    readFileSync(join(options.outDir, "queries.ts"), "utf8"),
    { overwrite: true },
  );
  if (
    !queriesSource
      .getExportDeclarations()
      .some(
        (declaration) =>
          declaration.getModuleSpecifierValue() === "./tanstack-query",
      )
  ) {
    queriesSource.addExportDeclaration({
      moduleSpecifier: "./tanstack-query",
    });
  }

  for (const file of [source, queriesSource]) {
    file.formatText();
    await file.save();
  }
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
