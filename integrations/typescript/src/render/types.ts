import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Project, QuoteKind, VariableDeclarationKind } from "ts-morph";
import type {
  FragmentManifestEntry,
  FragmentMetadata,
  FragmentSpreadMetadata,
  InputField,
  OperationManifestEntry,
  OperationMetadata,
  ResultField,
} from "../generated/metadata";
import type { BuildArtifacts } from "../node";

export type RenderOptions = {
  readonly outDir: string;
};

export type RenderDsqlOptions = {
  readonly root: string;
  readonly queriesDir: string;
  readonly executionDir?: string;
  readonly scope?: {
    readonly name: string;
    readonly imports?: readonly string[];
  };
};

export type DsqlRenderedFile = {
  readonly path: string;
  readonly contents: string;
};

export type DsqlRenderDefinitionResult = {
  readonly name: string;
  readonly kind: "query" | "fragment";
  readonly operationModule: string;
  readonly executionModule?: string;
};

export type DsqlRenderResult = {
  readonly scope?: {
    readonly name: string;
    readonly imports: readonly string[];
  };
  readonly modules: {
    readonly queries: string;
  };
  readonly definitions: Record<string, DsqlRenderDefinitionResult>;
  readonly files: readonly DsqlRenderedFile[];
};

const DEFINITION_KIND_FRAGMENT = "fragment";
const DEFINITION_KIND_QUERY = "query";
const RESULT_KIND_SCALAR = "scalar";
const RESULT_KIND_ARRAY = "array";
const PARAMS_PREFIX = "params";
const INPUT_PREFIX = "input";
const UNKNOWN_TS_TYPE = "unknown";

type InputRoot = typeof PARAMS_PREFIX | typeof INPUT_PREFIX;

export async function renderTypes(
  artifacts: BuildArtifacts,
  options: RenderOptions,
): Promise<void> {
  const project = createProject();
  const operationsSource = createSourceFromTemplate(
    project,
    options.outDir,
    "operations.ts",
  );

  for (const fragment of artifacts.fragments) {
    const resultType = fragmentResultTypeName(fragment.name);
    const paramsType = fragmentParamsTypeName(fragment.name);
    const inputType = fragmentInputTypeName(fragment.name);
    const variablesType = fragmentVariablesTypeName(fragment.name);
    operationsSource.addTypeAlias({
      isExported: true,
      name: resultType,
      type: resultTypeLiteral(fragment.result.fields, [], []),
    });
    operationsSource.addTypeAlias({
      isExported: true,
      name: paramsType,
      type: paramsTypeLiteral(fragment.params),
    });
    operationsSource.addTypeAlias({
      isExported: true,
      name: inputType,
      type: inputTypeLiteral(fragment.input),
    });
    operationsSource.addTypeAlias({
      isExported: true,
      name: variablesType,
      type: fragmentVariablesTypeLiteral(
        paramsType,
        inputType,
        fragment.params.length > 0,
        fragment.input.length > 0,
      ),
    });
    operationsSource.addVariableStatement({
      isExported: true,
      declarationKind: VariableDeclarationKind.Const,
      declarations: [
        {
          name: fragmentValueName(fragment.name),
          type: `DsqlFragmentDefinition<${resultType}, ${paramsType}, ${inputType}>`,
          initializer: `{
  name: ${JSON.stringify(fragment.name)},
  kind: ${JSON.stringify(DEFINITION_KIND_FRAGMENT)},
  table: ${JSON.stringify(fragment.table)}
}`,
        },
      ],
    });
  }

  for (const operation of artifacts.operations) {
    const manifestEntry = manifestEntryFor(artifacts, operation);
    const resultType = `${toPascalCase(operation.name)}Result`;
    const paramsType = `${toPascalCase(operation.name)}Params`;
    const inputType = `${toPascalCase(operation.name)}Input`;
    operationsSource.addTypeAlias({
      isExported: true,
      name: resultType,
      type: resultTypeLiteral(
        operation.result.fields,
        operation.fragment_spreads,
        artifacts.fragments,
      ),
    });
    operationsSource.addTypeAlias({
      isExported: true,
      name: paramsType,
      type: paramsTypeLiteral(operation.params),
    });
    operationsSource.addTypeAlias({
      isExported: true,
      name: inputType,
      type: inputTypeLiteral(
        operation.input,
        operation.fragment_spreads,
        artifacts.fragments,
      ),
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
  kind: ${JSON.stringify(DEFINITION_KIND_QUERY)}
}`,
        },
      ],
    });
  }

  operationsSource.addVariableStatement({
    isExported: true,
    declarationKind: VariableDeclarationKind.Const,
    declarations: [
      {
        name: "operations",
        initializer: `[
${artifacts.operations
  .map((operation) => `  ${toPascalCase(operation.name)}Operation`)
  .join(",\n")}
] as const`,
      },
    ],
  });

  await saveSourceFiles([operationsSource]);
}

export async function renderDsqlHelper(
  artifacts: BuildArtifacts,
  options: RenderOptions,
): Promise<void> {
  const project = createProject();
  const dsqlSource = createSourceFromTemplate(project, options.outDir, "dsql.ts");

  if (artifacts.operations.length > 0) {
    dsqlSource.addImportDeclaration({
      moduleSpecifier: "./operations",
      namedImports: artifacts.operations.map(
        (operation) => `${toPascalCase(operation.name)}Operation`,
      ),
    });
  }
  if (artifacts.fragments.length > 0) {
    dsqlSource.addImportDeclaration({
      moduleSpecifier: "./operations",
      namedImports: artifacts.fragments.map((fragment) =>
        fragmentValueName(fragment.name),
      ),
    });
  }

  dsqlSource.insertStatements(
    dsqlSource.getImportDeclarations().length,
    operationSourceMapType(artifacts),
  );

  const indexSource = createSourceFromTemplate(
    project,
    options.outDir,
    "index.ts",
  );
  const queriesSource = project.createSourceFile(
    join(options.outDir, "queries.ts"),
    'export * from "./index";\n',
    { overwrite: true },
  );

  await saveSourceFiles([dsqlSource, indexSource, queriesSource]);
}

export async function renderDsql(
  artifacts: BuildArtifacts,
  options: RenderDsqlOptions,
): Promise<DsqlRenderResult> {
  const root = resolve(options.root);
  const queriesDir = resolveOutputDir(root, options.queriesDir);
  const executionDir = options.executionDir
    ? resolveOutputDir(root, options.executionDir)
    : undefined;

  const renderPlan = buildRenderPlan(artifacts);
  const files = new Map<string, string>();
  const definitions: Record<string, DsqlRenderDefinitionResult> = {};
  const queryExports: string[] = [];
  const executionExports: string[] = [];

  for (const fragment of artifacts.fragments) {
    const plan = renderPlan.fragments.get(fragment.name);
    if (!plan) {
      throw new Error(`missing render plan for fragment ${fragment.name}`);
    }

    const filePath = join(queriesDir, `${plan.fileStem}.ts`);
    files.set(filePath, renderFragmentModule(artifacts, fragment));
    queryExports.push(exportStatement(plan.fileStem));
    definitions[fragment.name] = {
      name: fragment.name,
      kind: "fragment",
      operationModule: moduleSpecifier(root, filePath),
    };
  }

  for (const operation of artifacts.operations) {
    const plan = renderPlan.operations.get(operation.name);
    if (!plan) {
      throw new Error(`missing render plan for operation ${operation.name}`);
    }

    const operationPath = join(queriesDir, `${plan.fileStem}.ts`);
    const executionPath = executionDir
      ? join(executionDir, `${plan.fileStem}.ts`)
      : operationPath;
    files.set(
      operationPath,
      renderOperationModule(artifacts, operation, {
        includeExecutionPayload: executionDir === undefined,
      }),
    );
    queryExports.push(exportStatement(plan.fileStem));

    if (executionDir) {
      files.set(
        executionPath,
        renderOperationExecutionModule(operation, {
          operationImport: relativeModuleSpecifier(executionPath, operationPath),
        }),
      );
      executionExports.push(exportStatement(plan.fileStem));
    }

    definitions[operation.name] = {
      name: operation.name,
      kind: "query",
      operationModule: moduleSpecifier(root, operationPath),
      executionModule: moduleSpecifier(root, executionPath),
    };
  }

  files.set(join(queriesDir, "index.ts"), renderBarrel(queryExports));
  if (executionDir) {
    files.set(join(executionDir, "index.ts"), renderBarrel(executionExports));
  }

  const renderedFiles = [...files.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([path, contents]) => ({ path, contents }));

  for (const file of renderedFiles) {
    mkdirSync(dirname(file.path), { recursive: true });
    writeFileSync(file.path, file.contents);
  }

  return {
    ...(options.scope
      ? {
          scope: {
            name: options.scope.name,
            imports: [...(options.scope.imports ?? [])],
          },
        }
      : {}),
    modules: {
      queries: moduleSpecifier(root, join(queriesDir, "index.ts")),
    },
    definitions,
    files: renderedFiles,
  };
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
    readFileSync(join(packageRoot(), "templates", "bundled", name), "utf8"),
    { overwrite: true },
  );
}

async function saveSourceFiles(
  sourceFiles: ReturnType<Project["getSourceFiles"]>,
): Promise<void> {
  for (const sourceFile of sourceFiles) {
    sourceFile.formatText();
    await sourceFile.save();
  }
}

function packageRoot(): string {
  return dirname(dirname(dirname(fileURLToPath(import.meta.url))));
}

function manifestEntryFor(
  artifacts: BuildArtifacts,
  operation: OperationMetadata,
): OperationManifestEntry {
  const entry = artifacts.manifest.operations.find(
    (candidate) => candidate.name === operation.name,
  );
  if (!entry) {
    throw new Error(`missing manifest entry for operation ${operation.name}`);
  }
  return entry;
}

function fragmentManifestEntryFor(
  artifacts: BuildArtifacts,
  fragment: FragmentMetadata,
): FragmentManifestEntry {
  const entry = artifacts.manifest.fragments.find(
    (candidate) => candidate.name === fragment.name,
  );
  if (!entry) {
    throw new Error(`missing manifest entry for fragment ${fragment.name}`);
  }
  return entry;
}

function sourceTextForMap(
  artifacts: BuildArtifacts,
  definition: OperationMetadata | FragmentMetadata,
): string | undefined {
  const sourceMap = definition.source_map.find(
    (entry) => entry.id === definition.name,
  );
  if (!sourceMap) {
    return undefined;
  }

  const projectRoot = dirname(dirname(dirname(artifacts.manifestPath)));
  const source = readFileSync(join(projectRoot, sourceMap.file), "utf8");
  return (
    embeddedDsqlTextContainingRange(
      source,
      sourceMap.range.start,
      sourceMap.range.end,
    ) ?? source.slice(sourceMap.range.start, sourceMap.range.end)
  );
}

function renderOperationModule(
  artifacts: BuildArtifacts,
  operation: OperationMetadata,
  options: {
    readonly includeExecutionPayload: boolean;
  },
): string {
  const name = toPascalCase(operation.name);
  const manifestEntry = manifestEntryFor(artifacts, operation);
  const resultType = `${name}Result`;
  const paramsType = `${name}Params`;
  const inputType = `${name}Input`;
  const statements = [
    `import type { DsqlExecutionPayload, DsqlOperation } from "@dsql/typescript/runtime";`,
    "",
    `export type ${resultType} = ${resultTypeLiteral(
      operation.result.fields,
      operation.fragment_spreads,
      artifacts.fragments,
    )};`,
    "",
    `export type ${paramsType} = ${paramsTypeLiteral(operation.params)};`,
    "",
    `export type ${inputType} = ${inputTypeLiteral(
      operation.input,
      operation.fragment_spreads,
      artifacts.fragments,
    )};`,
    "",
    `export const ${name}Operation: DsqlOperation<${resultType}, ${paramsType}, ${inputType}> = {
  id: ${JSON.stringify(manifestEntry.hash)},
  name: ${JSON.stringify(operation.name)},
  kind: ${JSON.stringify(DEFINITION_KIND_QUERY)}
};`,
    "",
    renderSourceRegistryAugmentation(
      artifacts,
      operation,
      `${name}Operation`,
    ),
  ];

  if (options.includeExecutionPayload) {
    statements.push(
      "",
      renderExecutionPayload(operation, `${name}Operation`, {
        exportedName: `${name}ExecutionPayload`,
      }),
    );
  }

  return `${statements.join("\n")}\n`;
}

function renderOperationExecutionModule(
  operation: OperationMetadata,
  options: {
    readonly operationImport: string;
  },
): string {
  const name = toPascalCase(operation.name);
  return [
    `import type { DsqlExecutionPayload } from "@dsql/typescript/runtime";`,
    `import { ${name}Operation } from ${JSON.stringify(options.operationImport)};`,
    "",
    renderExecutionPayload(operation, `${name}Operation`, {
      exportedName: `${name}ExecutionPayload`,
    }),
    "",
  ].join("\n");
}

function renderExecutionPayload(
  operation: OperationMetadata,
  operationValue: string,
  options: {
    readonly exportedName: string;
  },
): string {
  return `export const ${options.exportedName}: DsqlExecutionPayload<typeof ${operationValue}> = {
  operation: ${operationValue},
  sql: ${JSON.stringify(operation.sql.text)},
  parameters: ${JSON.stringify(operation.sql.parameters)},
  variants: ${JSON.stringify(sqlVariants(operation))}
};`;
}

function renderFragmentModule(
  artifacts: BuildArtifacts,
  fragment: FragmentMetadata,
): string {
  const name = toPascalCase(fragment.name);
  const resultType = fragmentResultTypeName(fragment.name);
  const paramsType = fragmentParamsTypeName(fragment.name);
  const inputType = fragmentInputTypeName(fragment.name);
  const variablesType = fragmentVariablesTypeName(fragment.name);
  return [
    `import type { DsqlFragmentDefinition } from "@dsql/typescript/runtime";`,
    "",
    `export type ${resultType} = ${resultTypeLiteral(
      fragment.result.fields,
      [],
      [],
    )};`,
    "",
    `export type ${paramsType} = ${paramsTypeLiteral(fragment.params)};`,
    "",
    `export type ${inputType} = ${inputTypeLiteral(fragment.input)};`,
    "",
    `export type ${variablesType} = ${fragmentVariablesTypeLiteral(
      paramsType,
      inputType,
      fragment.params.length > 0,
      fragment.input.length > 0,
    )};`,
    "",
    `export const ${name}Fragment: DsqlFragmentDefinition<${resultType}, ${paramsType}, ${inputType}> = {
  name: ${JSON.stringify(fragment.name)},
  kind: ${JSON.stringify(DEFINITION_KIND_FRAGMENT)},
  table: ${JSON.stringify(fragment.table)}
};`,
    "",
    renderSourceRegistryAugmentation(artifacts, fragment, `${name}Fragment`),
    "",
  ].join("\n");
}

function renderSourceRegistryAugmentation(
  artifacts: BuildArtifacts,
  definition: OperationMetadata | FragmentMetadata,
  valueName: string,
): string {
  const sourceText = sourceTextForMap(artifacts, definition);
  if (!sourceText) {
    return "";
  }

  return `declare module "@dsql/typescript/runtime" {
  interface DsqlSourceRegistry {
    readonly ${JSON.stringify(sourceText)}: typeof ${valueName};
  }
}`;
}

function buildRenderPlan(artifacts: BuildArtifacts): {
  readonly operations: ReadonlyMap<
    string,
    { readonly fileStem: string; readonly exports: readonly string[] }
  >;
  readonly fragments: ReadonlyMap<
    string,
    { readonly fileStem: string; readonly exports: readonly string[] }
  >;
} {
  const fileStems = new Map<string, string>();
  const exportNames = new Map<string, string>();
  const operations = new Map<
    string,
    { readonly fileStem: string; readonly exports: readonly string[] }
  >();
  const fragments = new Map<
    string,
    { readonly fileStem: string; readonly exports: readonly string[] }
  >();

  for (const operation of artifacts.operations) {
    const name = toPascalCase(operation.name);
    const fileStem = name;
    const exports = [
      `${name}Result`,
      `${name}Params`,
      `${name}Input`,
      `${name}Operation`,
      `${name}ExecutionPayload`,
    ];
    recordGeneratedNames("operation", operation.name, fileStem, exports, {
      fileStems,
      exportNames,
    });
    operations.set(operation.name, { fileStem, exports });
  }

  for (const fragment of artifacts.fragments) {
    const name = toPascalCase(fragment.name);
    const fileStem = `${name}.fragment`;
    const exports = [
      fragmentResultTypeName(fragment.name),
      fragmentParamsTypeName(fragment.name),
      fragmentInputTypeName(fragment.name),
      fragmentVariablesTypeName(fragment.name),
      `${name}Fragment`,
    ];
    recordGeneratedNames("fragment", fragment.name, fileStem, exports, {
      fileStems,
      exportNames,
    });
    fragments.set(fragment.name, { fileStem, exports });
  }

  return { operations, fragments };
}

function recordGeneratedNames(
  kind: "operation" | "fragment",
  name: string,
  fileStem: string,
  exports: readonly string[],
  seen: {
    readonly fileStems: Map<string, string>;
    readonly exportNames: Map<string, string>;
  },
): void {
  const fileOwner = seen.fileStems.get(fileStem);
  const owner = `${kind} ${JSON.stringify(name)}`;
  if (fileOwner) {
    throw new Error(
      `generated DSQL file-stem collision for ${fileStem}: ${fileOwner} and ${owner}`,
    );
  }
  seen.fileStems.set(fileStem, owner);

  for (const exportName of exports) {
    const exportOwner = seen.exportNames.get(exportName);
    if (exportOwner) {
      throw new Error(
        `generated TypeScript export-name collision for ${exportName}: ${exportOwner} and ${owner}`,
      );
    }
    seen.exportNames.set(exportName, owner);
  }
}

function renderBarrel(exports: readonly string[]): string {
  return `${[...exports].sort().join("\n")}\n`;
}

function exportStatement(fileStem: string): string {
  return `export * from ${JSON.stringify(`./${fileStem}`)};`;
}

function resolveOutputDir(root: string, path: string): string {
  return isAbsolute(path) ? path : resolve(root, path);
}

function moduleSpecifier(root: string, path: string): string {
  const withoutExtension = path.replace(/\.ts$/, "");
  const relativePath = normalizeModulePath(relative(root, withoutExtension));
  return relativePath.startsWith(".") ? relativePath : `./${relativePath}`;
}

function relativeModuleSpecifier(fromFile: string, toFile: string): string {
  const fromDir = dirname(fromFile);
  const withoutExtension = toFile.replace(/\.ts$/, "");
  const relativePath = normalizeModulePath(relative(fromDir, withoutExtension));
  return relativePath.startsWith(".") ? relativePath : `./${relativePath}`;
}

function normalizeModulePath(path: string): string {
  return path.split("\\").join("/");
}

function sqlVariants(operation: OperationMetadata): Record<string, Record<string, string>> {
  return Object.fromEntries(
    operation.sql.variants.map((variant) => [
      variant.path,
      Object.fromEntries(
        variant.cases.map((case_) => [case_.value, case_.text]),
      ),
    ]),
  );
}

function operationSourceMapType(artifacts: BuildArtifacts): string {
  const definitionsBySource = new Map<string, Array<string>>();
  for (const operation of artifacts.operations) {
    const sourceText = sourceTextForMap(artifacts, operation);
    if (!sourceText) {
      continue;
    }

    const definitions = definitionsBySource.get(sourceText) ?? [];
    definitions.push(`typeof ${toPascalCase(operation.name)}Operation`);
    definitionsBySource.set(sourceText, definitions);
  }
  for (const fragment of artifacts.fragments) {
    fragmentManifestEntryFor(artifacts, fragment);
    const sourceText = sourceTextForMap(artifacts, fragment);
    if (!sourceText) {
      continue;
    }

    const definitions = definitionsBySource.get(sourceText) ?? [];
    definitions.push(`typeof ${fragmentValueName(fragment.name)}`);
    definitionsBySource.set(sourceText, definitions);
  }

  const entries = Array.from(definitionsBySource, ([sourceText, definitions]) => {
    return `  readonly ${JSON.stringify(sourceText)}: ${definitions.join(" | ")};`;
  });

  return `export type DsqlDefinitionBySource = {\n${entries.join("\n")}\n};`;
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

function resultTypeLiteral(
  fields: readonly ResultField[],
  fragmentSpreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): string {
  const roots = fields.filter(
    (field) => field.parent_path === "",
  );
  return objectType(
    roots.map((field) => propertyType(field, fields, fragmentSpreads, fragments)),
  );
}

function paramsTypeLiteral(fields: readonly InputField[]): string {
  return inputFieldsTypeLiteral(fields, PARAMS_PREFIX);
}

function inputTypeLiteral(
  fields: readonly InputField[],
  fragmentSpreads: readonly FragmentSpreadMetadata[] = [],
  fragments: readonly FragmentMetadata[] = [],
): string {
  return inputFieldsTypeLiteral(fields, INPUT_PREFIX, fragmentSpreads, fragments);
}

function inputFieldsTypeLiteral(
  fields: readonly InputField[],
  prefix: InputRoot,
  fragmentSpreads: readonly FragmentSpreadMetadata[] = [],
  fragments: readonly FragmentMetadata[] = [],
): string {
  if (fields.length === 0) {
    return "Record<string, never>";
  }

  const root = new TypeNode();
  const fragmentBranches =
    prefix === INPUT_PREFIX
      ? operationFragmentInputBranches(fields, fragmentSpreads, fragments)
      : [];
  for (const branch of fragmentBranches) {
    root.insert(branch.path, branch.type);
  }

  for (const field of fields) {
    const path = publicInputPath(field.path, prefix);
    if (path.length === 0) {
      continue;
    }
    if (fragmentBranches.some((branch) => pathStartsWith(path, branch.path))) {
      continue;
    }
    root.insert(path, inputFieldType(field));
  }
  return root.toTypeLiteral();
}

function operationFragmentInputBranches(
  fields: readonly InputField[],
  fragmentSpreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): Array<{ readonly path: readonly string[]; readonly type: string }> {
  if (fragmentSpreads.length === 0) {
    return [];
  }

  const fragmentNames = new Set(fragmentSpreads.map((spread) => spread.fragment));
  const branches = new Map<
    string,
    {
      path: string[];
      fragment: string;
      hasParams: boolean;
      hasInput: boolean;
    }
  >();

  for (const field of fields) {
    const path = publicInputPath(field.path, INPUT_PREFIX);
    for (let index = 0; index < path.length - 1; index += 1) {
      const fragment = path[index];
      const envelope = path[index + 1];
      if (
        fragment === undefined ||
        !fragmentNames.has(fragment) ||
        (envelope !== PARAMS_PREFIX && envelope !== INPUT_PREFIX)
      ) {
        continue;
      }

      const branchPath = path.slice(0, index + 1);
      const key = branchPath.join(".");
      const branch = branches.get(key) ?? {
        path: branchPath,
        fragment,
        hasParams: false,
        hasInput: false,
      };
      branch.hasParams ||= envelope === PARAMS_PREFIX;
      branch.hasInput ||= envelope === INPUT_PREFIX;
      branches.set(key, branch);
      break;
    }
  }

  return [...branches.values()]
    .filter((branch) =>
      fragments.some((fragment) => fragment.name === branch.fragment),
    )
    .map((branch) => ({
      path: branch.path,
      type: fragmentVariablesTypeName(branch.fragment),
    }));
}

function inputFieldType(field: InputField): string {
  const type =
    field.enum_values.length > 0
      ? field.enum_values.map((value) => JSON.stringify(value)).join(" | ")
      : dataType(field.data_type);
  return withNullability(type, field.nullable);
}

function publicInputPath(path: string, prefix: InputRoot): string[] {
  const parts = path.split(".").filter(Boolean);
  if (parts[0] !== prefix) {
    return parts;
  }
  return parts.slice(1);
}

function propertyType(
  field: ResultField,
  fields: readonly ResultField[],
  fragmentSpreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): [string, string] {
  if (field.kind === RESULT_KIND_SCALAR) {
    return [
      field.name,
      withNullability(dataType(field.data_type), field.nullable),
    ];
  }

  const children = fields.filter(
    (candidate) => candidate.parent_path === field.path,
  );
  const ownType = objectType(
    children.map((child) =>
      propertyType(child, fields, fragmentSpreads, fragments),
    ),
  );
  const spreadTypes = fragmentSpreads
    .filter((spread) => spread.path === field.path)
    .map((spread) => fragmentResultTypeName(spread.fragment));
  const type = composeObjectType(
    spreadTypes,
    ownType,
    objectHasOwnFields(field, fields, fragmentSpreads, fragments),
  );
  const resultType = field.kind === RESULT_KIND_ARRAY ? `Array<${type}>` : type;
  return [field.name, withNullability(resultType, field.nullable)];
}

function objectHasOwnFields(
  field: ResultField,
  fields: readonly ResultField[],
  fragmentSpreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): boolean {
  const providedPaths = fragmentProvidedPaths(field.path, fragmentSpreads, fragments);
  if (providedPaths.size === 0) {
    return fields.some((candidate) => candidate.parent_path === field.path);
  }
  return fields.some((candidate) => {
    const relativePath = relativeResultPath(field.path, candidate.path);
    return relativePath !== undefined && !providedPaths.has(relativePath);
  });
}

function fragmentProvidedPaths(
  path: string,
  fragmentSpreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): ReadonlySet<string> {
  const provided = new Set<string>();
  for (const spread of fragmentSpreads) {
    if (spread.path !== path) {
      continue;
    }
    const fragment = fragments.find((candidate) => candidate.name === spread.fragment);
    for (const field of fragment?.result.fields ?? []) {
      provided.add(field.path);
    }
  }
  return provided;
}

function relativeResultPath(
  parentPath: string,
  path: string,
): string | undefined {
  const prefix = `${parentPath}.`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : undefined;
}

function pathStartsWith(path: readonly string[], prefix: readonly string[]): boolean {
  return prefix.every((part, index) => path[index] === part);
}

function composeObjectType(
  spreadTypes: readonly string[],
  ownType: string,
  hasOwnFields: boolean,
): string {
  if (spreadTypes.length === 0) {
    return ownType;
  }
  if (!hasOwnFields) {
    return spreadTypes.join(" & ");
  }
  return [...spreadTypes, ownType].join(" & ");
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

function objectTypeWithOptional(
  properties: Array<{
    readonly name: string;
    readonly type: string;
    readonly optional: boolean;
  }>,
): string {
  if (properties.length === 0) {
    return "Record<string, never>";
  }

  return `{
${properties
  .map(
    (property) =>
      `  ${propertyName(property.name)}${property.optional ? "?" : ""}: ${property.type};`,
  )
  .join("\n")}
}`;
}

function fragmentVariablesTypeLiteral(
  paramsType: string,
  inputType: string,
  hasParams: boolean,
  hasInput: boolean,
): string {
  return objectTypeWithOptional([
    { name: PARAMS_PREFIX, type: paramsType, optional: !hasParams },
    { name: INPUT_PREFIX, type: inputType, optional: !hasInput },
  ]);
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
      return this.value ?? UNKNOWN_TS_TYPE;
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
      return UNKNOWN_TS_TYPE;
    case "text":
    case "timestamptz":
    case "uuid":
      return "string";
    default:
      return UNKNOWN_TS_TYPE;
  }
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

function fragmentValueName(name: string): string {
  return `${toPascalCase(name)}Fragment`;
}

function fragmentResultTypeName(name: string): string {
  return `${toPascalCase(name)}FragmentResult`;
}

function fragmentParamsTypeName(name: string): string {
  return `${toPascalCase(name)}FragmentParams`;
}

function fragmentInputTypeName(name: string): string {
  return `${toPascalCase(name)}FragmentInput`;
}

function fragmentVariablesTypeName(name: string): string {
  return `${toPascalCase(name)}FragmentVariables`;
}
