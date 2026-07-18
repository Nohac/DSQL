import { join, resolve } from "node:path";
import { artifactKey } from "@dsql/typescript/node";
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
  readonly validatorFor?: DsqlValidatorResolver;
};

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

export async function renderTanStackStart(
  artifacts: BuildArtifacts,
  dsql: DsqlRenderResult,
  options: RenderOptions,
): Promise<string[]> {
  const root = resolve(options.root ?? process.cwd());
  const clientPath = join(options.outDir, "tanstack-start.ts");
  const serverPath = join(options.outDir, "tanstack-start.server.ts");
  const project = createGeneratorProject();
  const source = createSourceFromTemplate(
    project,
    options.outDir,
    "tanstack-start.ts",
    import.meta.url,
  );
  const serverSource = project.createSourceFile(
    serverPath,
    'import "@tanstack/react-start/server-only";\n',
    { overwrite: true },
  );

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
        namedImports: [
          `${toPascalCase(operation.name)}ExecutionPayload`,
        ],
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

  for (const file of [source, serverSource]) {
    file.formatText();
    await file.save();
  }
  return [clientPath, serverPath];
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
