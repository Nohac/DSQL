import { mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Project, QuoteKind, VariableDeclarationKind } from "ts-morph";
import type {
  InputField,
  OperationManifestEntry,
  OperationMetadata,
  ResultField,
} from "@dsql/typescript";
import { loadBuildArtifacts } from "@dsql/typescript/node";

const manifestPath = process.env.DSQL_MANIFEST;
const outDir = process.env.DSQL_OUT_DIR;

if (!manifestPath || !outDir) {
  throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
}

const manifestFile = manifestPath;
const outputDirectory = outDir;
const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const artifacts = loadBuildArtifacts(manifestFile);
const { operations } = artifacts;
const projectRoot = dirname(dirname(dirname(manifestFile)));

mkdirSync(outputDirectory, { recursive: true });

const project = new Project({
  manipulationSettings: {
    quoteKind: QuoteKind.Double,
  },
});

const operationsSource = createSourceFromTemplate("operations.ts");
const dsqlSource = createSourceFromTemplate("dsql.ts");
const tanstackQuerySource = createSourceFromTemplate("tanstack-query.ts");
const indexSource = createSourceFromTemplate("index.ts");
const queriesSource = project.createSourceFile(
  join(outputDirectory, "queries.ts"),
  'export * from "./index";\n',
  { overwrite: true },
);

for (const operation of operations) {
  const manifestEntry = manifestEntryFor(operation);
  const resultType = `${toPascalCase(operation.name)}Result`;
  const paramsType = `${toPascalCase(operation.name)}Params`;
  const inputType = `${toPascalCase(operation.name)}Input`;
  operationsSource.addTypeAlias({
    isExported: true,
    name: resultType,
    type: resultTypeLiteral(operation),
  });
  operationsSource.addTypeAlias({
    isExported: true,
    name: paramsType,
    type: paramsTypeLiteral(operation.params),
  });
  operationsSource.addTypeAlias({
    isExported: true,
    name: inputType,
    type: inputTypeLiteral(operation.input),
  });
  operationsSource.addVariableStatement({
    isExported: true,
    declarationKind: VariableDeclarationKind.Const,
    declarations: [
      {
        name: `${toPascalCase(operation.name)}Operation`,
        type: `DsqlOperation<${resultType}, ${paramsType}, ${inputType}>`,
        initializer: `{
  id: ${JSON.stringify(manifestEntry.hash)},
  name: ${JSON.stringify(operation.name)},
  kind: "query",
  sql: ${JSON.stringify(operation.sql.text)}
}`,
      },
    ],
  });
}

if (operations.length > 0) {
  dsqlSource.addImportDeclaration({
    moduleSpecifier: "./operations",
    namedImports: operations.map(
      (operation) => `${toPascalCase(operation.name)}Operation`,
    ),
  });
}

const dsqlFunctionIndex = dsqlSource
  .getStatements()
  .findIndex((statement) => statement.getText().startsWith("export function dsql("));
if (dsqlFunctionIndex === -1) {
  throw new Error("dsql template must define a dsql function");
}
dsqlSource.insertStatements(dsqlFunctionIndex, operationSourceMapType());

operationsSource.addVariableStatement({
  isExported: true,
  declarationKind: VariableDeclarationKind.Const,
  declarations: [
    {
      name: "operations",
      initializer: `[
${operations
  .map((operation) => `  ${toPascalCase(operation.name)}Operation`)
  .join(",\n")}
] as const`,
    },
  ],
});

for (const sourceFile of [
  operationsSource,
  dsqlSource,
  tanstackQuerySource,
  indexSource,
  queriesSource,
]) {
  sourceFile.formatText();
  sourceFile.saveSync();
}

function createSourceFromTemplate(name: string) {
  return project.createSourceFile(
    join(outputDirectory, name),
    readFileSync(join(packageRoot, "templates", name), "utf8"),
    { overwrite: true },
  );
}

function manifestEntryFor(operation: OperationMetadata): OperationManifestEntry {
  const entry = artifacts.manifest.operations.find(
    (candidate) => candidate.name === operation.name,
  );
  if (!entry) {
    throw new Error(`missing manifest entry for operation ${operation.name}`);
  }
  return entry;
}

function operationSourceText(operation: OperationMetadata): string | undefined {
  const sourceMap = operation.source_map.find(
    (entry) => entry.id === operation.name,
  );
  if (!sourceMap) {
    return undefined;
  }

  const source = readFileSync(join(projectRoot, sourceMap.file), "utf8");
  return (
    embeddedDsqlTextContainingRange(
      source,
      sourceMap.range.start,
      sourceMap.range.end,
    ) ?? source.slice(sourceMap.range.start, sourceMap.range.end)
  );
}

function operationSourceMapType(): string {
  const entries = operations
    .map((operation) => {
      const sourceText = operationSourceText(operation);
      if (!sourceText) {
        return undefined;
      }
      return `  readonly ${JSON.stringify(sourceText)}: typeof ${toPascalCase(
        operation.name,
      )}Operation;`;
    })
    .filter((entry): entry is string => entry !== undefined);

  return `export type DsqlOperationBySource = {\n${entries.join("\n")}\n};`;
}

function embeddedDsqlTextContainingRange(
  source: string,
  start: number,
  end: number,
): string | undefined {
  const pattern = /dsql(?:\s*\(\s*)?`(?<content>[\s\S]*?)`(?:\s*\))?/g;
  for (const match of source.matchAll(pattern)) {
    const content = match.groups?.content;
    if (content === undefined || match.index === undefined) {
      continue;
    }

    const contentOffset = match[0].indexOf(content);
    if (contentOffset < 0) {
      continue;
    }

    const contentStart = match.index + contentOffset;
    const contentEnd = contentStart + content.length;
    if (start >= contentStart && end <= contentEnd) {
      return content;
    }
  }
  return undefined;
}

function resultTypeLiteral(operation: OperationMetadata): string {
  const roots = operation.result.fields.filter(
    (field) => field.parent_path === "",
  );
  return objectType(
    roots.map((field) => propertyType(field, operation.result.fields)),
  );
}

function paramsTypeLiteral(fields: readonly InputField[]): string {
  return inputFieldsTypeLiteral(fields, "params");
}

function inputTypeLiteral(fields: readonly InputField[]): string {
  return inputFieldsTypeLiteral(fields, "input");
}

function inputFieldsTypeLiteral(
  fields: readonly InputField[],
  prefix: "params" | "input",
): string {
  if (fields.length === 0) {
    return "Record<string, never>";
  }

  const root = new TypeNode();
  for (const field of fields) {
    const path = publicInputPath(field.path, prefix);
    if (path.length === 0) {
      continue;
    }
    root.insert(path, withNullability(dataType(field.data_type), field.nullable));
  }
  return root.toTypeLiteral();
}

function publicInputPath(path: string, prefix: "params" | "input"): string[] {
  const parts = path.split(".").filter(Boolean);
  if (parts[0] !== prefix) {
    return parts;
  }
  return parts.slice(1);
}

function propertyType(
  field: ResultField,
  fields: readonly ResultField[],
): [string, string] {
  if (field.kind === "scalar") {
    return [
      field.name,
      withNullability(dataType(field.data_type), field.nullable),
    ];
  }

  const children = fields.filter(
    (candidate) => candidate.parent_path === field.path,
  );
  const type = objectType(children.map((child) => propertyType(child, fields)));
  return [field.name, field.kind === "array" ? `Array<${type}>` : type];
}

function objectType(properties: Array<[string, string]>): string {
  if (properties.length === 0) {
    return "Record<string, never>";
  }

  return `{
${properties
  .map(([name, type]) => `  ${propertyName(name)}: ${type};`)
  .join("\n")}
}`;
}

class TypeNode {
  private readonly children = new Map<string, TypeNode>();
  private value: string | undefined;

  insert(path: readonly string[], type: string): void {
    if (path.length === 0) {
      this.value = type;
      return;
    }

    const [head, ...tail] = path;
    if (head === undefined) {
      this.value = type;
      return;
    }
    let child = this.children.get(head);
    if (!child) {
      child = new TypeNode();
      this.children.set(head, child);
    }
    child.insert(tail, type);
  }

  toTypeLiteral(): string {
    if (this.children.size === 0) {
      return this.value ?? "unknown";
    }

    return objectType(
      [...this.children.entries()].map(([name, child]) => [
        name,
        child.toTypeLiteral(),
      ]),
    );
  }
}

function propertyName(name: string): string {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? name : JSON.stringify(name);
}

function withNullability(type: string, nullable: boolean): string {
  return nullable ? `${type} | null` : type;
}

function dataType(type: string): string {
  switch (type) {
    case "boolean":
      return "boolean";
    case "int":
      return "number";
    case "json":
      return "unknown";
    case "text":
    case "timestamptz":
    case "uuid":
      return "string";
    default:
      return "unknown";
  }
}

function toPascalCase(value: string): string {
  const result = value
    .split(/[^A-Za-z0-9]+/)
    .filter(Boolean)
    .map((part) => `${part[0]?.toUpperCase() ?? ""}${part.slice(1)}`)
    .join("");

  if (!result) {
    return "Operation";
  }

  return /^[0-9]/.test(result) ? `_${result}` : result;
}
