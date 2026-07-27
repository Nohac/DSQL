import { join, resolve } from "node:path";
import { artifactKey } from "@dsql/typescript/node";
import type { BuildArtifacts } from "@dsql/typescript/node";
import {
  VariableDeclarationKind,
  type DsqlProjectGenerator,
} from "@dsql/typescript/renderer";
import {
  createGeneratorProject,
  createSourceFromTemplate,
  importSpecifier,
  toPascalCase,
} from "./shared";

type RenderOptions = {
  readonly validatorFor?: DsqlValidatorResolver;
};

/**
 * Resolves a validator for the serialized `DsqlWireVariables` accepted by
 * the generated server function.
 */
export type DsqlValidatorResolver = (
  operation: BuildArtifacts["operations"][number],
) => DsqlValidatorExpression | "identity" | undefined;

export type DsqlValidatorExpression = {
  readonly import?: {
    readonly name: string;
    readonly from: string;
  };
  readonly expression: string;
};

export function tanstackStart(
  options: RenderOptions = {},
): Omit<DsqlProjectGenerator<string>, "targets"> {
  return {
    name: "tanstack-start",
    render(context) {
      const root = resolve(context.projectBase);
      const outDir = resolve(root, context.outputDirectory);
      const clientPath = join(outDir, "tanstack-start.ts");
      const serverPath = join(outDir, "tanstack-start.server.ts");
      const project = createGeneratorProject();
      const source = createSourceFromTemplate(
        project,
        outDir,
        "tanstack-start.ts",
        import.meta.url,
      );
      const serverSource = project.createSourceFile(
        serverPath,
        'import "@tanstack/react-start/server-only";\n',
        { overwrite: true },
      );
      const artifacts = context.artifacts;
      const dsql = context.definitions.current;

      if (artifacts.operations.length > 0) {
        source.addImportDeclaration({
          moduleSpecifier: importSpecifier(root, clientPath, dsql.modules.queries),
          namedImports: artifacts.operations.map(
            (operation) => `${toPascalCase(operation.name)}Operation`,
          ),
        });
        for (const operation of artifacts.operations) {
          const definition = dsql.definitions[artifactKey("operation", operation.name)];
          if (!definition?.executionModule) {
            throw new Error(`missing DSQL execution module for ${operation.name}`);
          }
          serverSource.addImportDeclaration({
            moduleSpecifier: importSpecifier(
              root,
              serverPath,
              definition.executionModule,
            ),
            namedImports: [`${toPascalCase(operation.name)}ExecutionPayload`],
          });
        }
      }
      for (const validator of validatorImports(artifacts, options.validatorFor)) {
        source.addImportDeclaration({
          moduleSpecifier: validator.from,
          namedImports: [validator.name],
        });
      }

      serverSource.addImportDeclaration({
        moduleSpecifier: "@dsql/typescript/runtime",
        isTypeOnly: true,
        namedImports: ["DsqlExecutionPayload"],
      });
      source.addVariableStatement({
        isExported: true,
        declarationKind: VariableDeclarationKind.Const,
        declarations: [
          {
            name: "serverOperationNames",
            initializer: `${JSON.stringify(
              artifacts.operations.map((operation) => operation.name),
            )} as const`,
          },
        ],
      });

      for (const operation of artifacts.operations) {
        const operationName = `${toPascalCase(operation.name)}Operation`;
        source.addVariableStatement({
          isExported: true,
          declarationKind: VariableDeclarationKind.Const,
          declarations: [
            {
              name: `${toPascalCase(operation.name)}ServerFn`,
              initializer: `createServerFn({ method: "POST", strict: { output: false } })
  .inputValidator(${validatorExpression(operation, operationName, options.validatorFor)})
  .handler(({ data }) => executeDsqlOperation(${operationName}, data))`,
            },
          ],
        });
      }
      serverSource.addVariableStatement({
        isExported: true,
        declarationKind: VariableDeclarationKind.Const,
        declarations: [
          {
            name: "executionPayloads",
            type: "Record<string, DsqlExecutionPayload<any>>",
            initializer: `{
${artifacts.operations
  .map((operation) => {
    const name = toPascalCase(operation.name);
    return `  ${JSON.stringify(operation.name)}: ${name}ExecutionPayload`;
  })
  .join(",\n")}
}`,
          },
        ],
      });

      source.formatText();
      serverSource.formatText();
      context.files.write("tanstack-start.ts", source.getFullText());
      context.files.write("tanstack-start.server.ts", serverSource.getFullText());
    },
  };
}

function validatorImports(
  artifacts: BuildArtifacts,
  validatorFor: DsqlValidatorResolver | undefined,
): Array<{ readonly name: string; readonly from: string }> {
  const imports = new Map<string, { readonly name: string; readonly from: string }>();
  for (const operation of artifacts.operations) {
    const validator = validatorFor?.(operation);
    if (!validator || validator === "identity" || !validator.import) {
      continue;
    }
    imports.set(`${validator.import.from}:${validator.import.name}`, validator.import);
  }
  return [...imports.values()];
}

function validatorExpression(
  operation: BuildArtifacts["operations"][number],
  operationName: string,
  validatorFor: DsqlValidatorResolver | undefined,
): string {
  const validator = validatorFor?.(operation);
  if (!validator || validator === "identity") {
    return `(variables: DsqlServerVariables<typeof ${operationName}>) => variables`;
  }

  return `${validator.expression} as (variables: DsqlServerVariables<typeof ${operationName}>) => DsqlServerVariables<typeof ${operationName}>`;
}
