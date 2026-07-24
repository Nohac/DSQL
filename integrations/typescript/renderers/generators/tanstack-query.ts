import { join, resolve } from "node:path";
import type { DsqlProjectGenerator } from "@dsql/typescript/renderer";
import { VariableDeclarationKind } from "@dsql/typescript/renderer";
import {
  createGeneratorProject,
  createSourceFromTemplate,
  importSpecifier,
  toPascalCase,
} from "./shared";

export function tanstackQuery(): Omit<DsqlProjectGenerator<string>, "targets"> {
  return {
    name: "tanstack-query",
    render(context) {
      const root = resolve(context.projectBase);
      const outDir = resolve(root, context.outputDirectory);
      const sourcePath = join(outDir, "tanstack-query.ts");
      const project = createGeneratorProject();
      const source = createSourceFromTemplate(
        project,
        outDir,
        "tanstack-query.ts",
        import.meta.url,
      );
      const artifacts = context.artifacts;
      const dsql = context.definitions.current;

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
      context.files.write("tanstack-query.ts", source.getFullText());
    },
  };
}
