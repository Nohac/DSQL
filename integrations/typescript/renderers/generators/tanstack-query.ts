import { join, resolve } from "node:path";
import type { BuildArtifacts, DsqlRenderResult } from "@dsql/typescript/node";
import { VariableDeclarationKind } from "@dsql/typescript/renderer";
import {
  createGeneratorProject,
  createSourceFromTemplate,
  importSpecifier,
  toPascalCase,
} from "./shared";

type RenderOptions = {
  readonly outDir: string;
  readonly root?: string;
};

export async function renderTanStackQuery(
  artifacts: BuildArtifacts,
  dsql: DsqlRenderResult,
  options: RenderOptions,
): Promise<string[]> {
  const root = resolve(options.root ?? process.cwd());
  const sourcePath = join(options.outDir, "tanstack-query.ts");
  const project = createGeneratorProject();
  const source = createSourceFromTemplate(
    project,
    options.outDir,
    "tanstack-query.ts",
    import.meta.url,
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
  return [sourcePath];
}
