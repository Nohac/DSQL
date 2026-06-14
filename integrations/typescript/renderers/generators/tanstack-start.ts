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
): Promise<void> {
  const root = resolve(options.root ?? process.cwd());
  const clientPath = join(options.outDir, "tanstack-start.ts");
  const serverPath = join(options.outDir, "tanstack-start.server.ts");
  const project = createProject();
  const source = createSourceFromTemplate(
    project,
    options.outDir,
    "tanstack-start.ts",
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
      const definition = dsql.definitions[operation.name];
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
