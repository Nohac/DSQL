import { mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Project, QuoteKind, VariableDeclarationKind } from "ts-morph";
import { loadBuildArtifacts } from "../src/node.js";
import type { OperationMetadata, ResultField } from "../src/index.js";

const manifestPath = process.env.DSQL_MANIFEST;
const outDir = process.env.DSQL_OUT_DIR;

if (!manifestPath || !outDir) {
  throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
}

const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const templatePath = join(packageRoot, "templates", "queries.ts");
const { operations } = loadBuildArtifacts(manifestPath);

mkdirSync(outDir, { recursive: true });

const project = new Project({
  manipulationSettings: {
    quoteKind: QuoteKind.Double,
  },
});

const source = project.createSourceFile(
  join(outDir, "queries.ts"),
  readFileSync(templatePath, "utf8"),
  { overwrite: true },
);

for (const operation of operations) {
  const resultType = `${toPascalCase(operation.name)}Result`;
  source.addTypeAlias({
    isExported: true,
    name: resultType,
    type: resultTypeLiteral(operation),
  });
  source.addVariableStatement({
    isExported: true,
    declarationKind: VariableDeclarationKind.Const,
    declarations: [
      {
        name: `${toPascalCase(operation.name)}Operation`,
        initializer: `{
  name: ${JSON.stringify(operation.name)},
  kind: "query",
  sql: ${JSON.stringify(operation.sql.text)}
} as const satisfies DsqlOperation<${resultType}>`,
      },
    ],
  });
}

source.addVariableStatement({
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

source.formatText();
source.saveSync();

function resultTypeLiteral(operation: OperationMetadata): string {
  const roots = operation.result.fields.filter(
    (field) => field.parent_path === "",
  );
  return objectType(
    roots.map((field) => propertyType(field, operation.result.fields)),
  );
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
