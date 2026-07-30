import { readFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import ts from "typescript";
import type {
  ClosedValueSetMetadata,
  DynamicInputMetadata,
  FragmentMetadata,
  FragmentSpreadMetadata,
  InputField,
  OperationManifestEntry,
  OperationMetadata,
  ResultField,
  WireEncoding,
} from "../generated/metadata.ts";
import type { BuildArtifacts } from "../node.ts";
import type { DsqlOutputMode } from "../node.ts";

export type RenderDsqlOptions = {
  readonly root: string;
  readonly queriesDir: string;
  readonly executionDir?: string;
  readonly scope?: {
    readonly name: string;
    readonly imports?: readonly string[];
  };
  /**
   * `family/name` → exact embedded template content, resolved by the
   * caller through Rust-owned `content_range` slices (see
   * `resolveEmbeddedSources`). Operations present here get a
   * `DsqlSourceRegistry` augmentation keyed by that content.
   */
  readonly embeddedSources?: ReadonlyMap<string, string>;
  /** Generated-source presentation; defaults to readable. */
  readonly outputMode?: DsqlOutputMode;
  /** Project-owned host representations keyed by logical scalar name. */
  readonly scalars?: DsqlScalarMappings;
};

/** One named export that generated TypeScript imports directly. */
export type DsqlNamedReference = {
  readonly from: string;
  readonly name: string;
};

/**
 * One host representation for a compiler logical scalar. Codec hooks are
 * paired because a logical type has one representation in both directions.
 */
export type DsqlScalarMapping = {
  readonly type: DsqlNamedReference;
  readonly parse?: DsqlNamedReference;
  readonly serialize?: DsqlNamedReference;
};

/** Logical scalar name to project-owned TypeScript representation. */
export type DsqlScalarMappings = Readonly<Record<string, DsqlScalarMapping>>;

export type DsqlRenderedFile = {
  readonly path: string;
  readonly contents: string;
};

export type DsqlRenderDefinitionResult = {
  readonly name: string;
  readonly kind: "query" | "fragment";
  /** Full artifact id when the artifacts carried one (daemon channel). */
  readonly id?: string;
  /** The definition's exported const (`<Name>Operation`/`<Name>Fragment`). */
  readonly exportName: string;
  /** Root-relative import specifier (renderer-internal consumers). */
  readonly operationModule: string;
  /** Project-base-relative file path (render-map contract). */
  readonly modulePath: string;
  readonly executionModule?: string;
};

/** The key `embeddedSources` and `BuildArtifacts.artifactIds` use. */
export function artifactKey(kind: "operation" | "fragment", name: string): string {
  return `${kind}/${name}`;
}

type ClosedValueCarrier = {
  readonly closed_values: ClosedValueSetMetadata;
};

function hasClosedValues(value: ClosedValueCarrier): boolean {
  return value.closed_values.values.length > 0;
}

function closedValueType(value: ClosedValueCarrier): string {
  return value.closed_values.values
    .map((entry) => JSON.stringify(entry.value))
    .join(" | ");
}

type ScalarOccurrence = {
  readonly name: string;
  readonly wire: WireEncoding;
};

/** Validates project-owned scalar mappings against one complete artifact view. */
export function validateDsqlScalarMappings(
  artifacts: BuildArtifacts,
  mappings: DsqlScalarMappings,
): void {
  const occurrences = scalarOccurrences(artifacts);
  for (const [logicalName, mapping] of Object.entries(mappings)) {
    if (SPECIAL_INPUT_TYPES.has(logicalName) || logicalName === "object") {
      throw new Error(
        `dsql scalar mapping ${logicalName} names a structural compiler type`,
      );
    }
    validateNamedReference(mapping.type, `${logicalName}.type`);
    if ((mapping.parse === undefined) !== (mapping.serialize === undefined)) {
      throw new Error(
        `dsql scalar mapping ${logicalName} must declare parse and serialize together`,
      );
    }
    if (mapping.parse) {
      validateNamedReference(mapping.parse, `${logicalName}.parse`);
    }
    if (mapping.serialize) {
      validateNamedReference(mapping.serialize, `${logicalName}.serialize`);
    }

    const matching = occurrences.filter(
      (occurrence) => occurrence.name === logicalName,
    );
    if (matching.length === 0) {
      throw new Error(
        `dsql scalar mapping ${logicalName} does not match any generated logical type`,
      );
    }
    if (matching.some((occurrence) => occurrence.wire === "unsupported")) {
      throw new Error(
        `dsql scalar mapping ${logicalName} targets an unsupported wire encoding`,
      );
    }
    const wires = new Set(matching.map((occurrence) => occurrence.wire));
    if (wires.size > 1) {
      throw new Error(
        `dsql scalar mapping ${logicalName} spans inconsistent wire encodings: ${[
          ...wires,
        ]
          .sort()
          .join(", ")}`,
      );
    }
  }
}

/**
 * Keeps globally validated mappings that one target's artifact closure uses.
 * Shared renderer configuration therefore does not false-reject a target that
 * happens not to use another target's scalar.
 */
export function scalarMappingsForArtifacts(
  artifacts: BuildArtifacts,
  mappings: DsqlScalarMappings,
): DsqlScalarMappings {
  const names = new Set(
    scalarOccurrences(artifacts).map((occurrence) => occurrence.name),
  );
  return Object.fromEntries(
    Object.entries(mappings).filter(([name]) => names.has(name)),
  );
}

function scalarOccurrences(artifacts: BuildArtifacts): ScalarOccurrence[] {
  const occurrences: ScalarOccurrence[] = [];
  const inputs = (fields: readonly InputField[]) => {
    for (const field of fields) {
      if (
        !SPECIAL_INPUT_TYPES.has(field.data_type) &&
        !hasClosedValues(field)
      ) {
        occurrences.push({
          name: field.data_type,
          wire: field.wire.encoding,
        });
      }
    }
  };
  const results = (fields: readonly ResultField[]) => {
    for (const field of fields) {
      if (
        field.kind === RESULT_KIND_SCALAR &&
        !hasClosedValues(field.value_type)
      ) {
        occurrences.push({
          name: field.value_type.name,
          wire: field.value_type.wire.encoding,
        });
      }
    }
  };
  for (const operation of artifacts.operations) {
    inputs([...operation.params, ...operation.input, ...operation.context]);
    for (const dynamic of operation.dynamic_inputs) {
      if (dynamic.kind !== "predicate") {
        continue;
      }
      for (const field of dynamic.fields) {
        if (hasClosedValues(field)) {
          continue;
        }
        occurrences.push({
          name: field.data_type,
          wire: field.wire.encoding,
        });
      }
    }
    results(operation.result.fields);
  }
  for (const fragment of artifacts.fragments) {
    inputs([...fragment.params, ...fragment.input]);
    for (const dynamic of fragment.dynamic_inputs) {
      if (dynamic.kind !== "predicate") {
        continue;
      }
      for (const field of dynamic.fields) {
        if (hasClosedValues(field)) {
          continue;
        }
        occurrences.push({
          name: field.data_type,
          wire: field.wire.encoding,
        });
      }
    }
    results(fragment.result.fields);
  }
  return occurrences;
}

function validateNamedReference(
  reference: DsqlNamedReference,
  path: string,
): void {
  if (reference.from.trim().length === 0) {
    throw new Error(`dsql scalar mapping ${path}.from must not be empty`);
  }
  if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(reference.name)) {
    throw new Error(
      `dsql scalar mapping ${path}.name ${JSON.stringify(
        reference.name,
      )} is not an identifier`,
    );
  }
}

export type DsqlRenderResult = {
  readonly scope?: {
    readonly name: string;
    readonly imports: readonly string[];
  };
  readonly modules: {
    readonly queries: string;
  };
  /** `family/name` → rendered definition result. */
  readonly definitions: Record<string, DsqlRenderDefinitionResult>;
  readonly files: readonly DsqlRenderedFile[];
};

const DEFINITION_KIND_FRAGMENT = "fragment";
const DEFINITION_KIND_QUERY = "query";
const RESULT_KIND_SCALAR = "scalar";
const RESULT_KIND_ARRAY = "array";
const RESULT_VALUE_SHAPE_DATABASE_ARRAY = "database_array";
const PARAMS_PREFIX = "params";
const INPUT_PREFIX = "input";
const CONTEXT_PREFIX = "context";
const FLOAT_TS_TYPE = 'number | "NaN" | "Infinity" | "-Infinity"';
const UNKNOWN_TS_TYPE = "unknown";
const SPECIAL_INPUT_TYPES = new Set(["dynamic_predicate", "dynamic_order"]);

type ScalarDirection = "input" | "result";
type ScalarRole = "type" | "parse" | "serialize";

class ScalarModuleContext {
  readonly #mappings: DsqlScalarMappings;
  readonly #imports = new Set<string>();
  #usesParser = false;
  #usesSerializer = false;

  constructor(mappings: DsqlScalarMappings) {
    this.#mappings = mappings;
  }

  hasMapping(logicalName: string): boolean {
    return this.#mappings[logicalName] !== undefined;
  }

  typeFor(
    logicalName: string,
    wire: WireEncoding,
    direction: ScalarDirection,
    mapped = true,
  ): string {
    const mapping = mapped ? this.#mappings[logicalName] : undefined;
    if (!mapping) {
      return wireType(wire, direction);
    }
    const alias = scalarAlias(logicalName, "type");
    this.#imports.add(
      `import type { ${mapping.type.name} as ${alias} } from ${JSON.stringify(mapping.type.from)};`,
    );
    return alias;
  }

  serializerFor(
    logicalName: string,
    wire: WireEncoding,
    mapped = true,
  ): string | undefined {
    const mapping = mapped ? this.#mappings[logicalName] : undefined;
    if (!mapping?.serialize) {
      return undefined;
    }
    const hostType = this.typeFor(logicalName, wire, "input");
    const alias = scalarAlias(logicalName, "serialize");
    this.#imports.add(
      `import { ${mapping.serialize.name} as ${alias} } from ${JSON.stringify(mapping.serialize.from)};`,
    );
    this.#usesSerializer = true;
    return `${alias} satisfies DsqlScalarSerializer<${hostType}, ${wireType(wire, "input")}>`;
  }

  parserFor(logicalName: string, wire: WireEncoding): string | undefined {
    const mapping = this.#mappings[logicalName];
    if (!mapping?.parse) {
      return undefined;
    }
    const hostType = this.typeFor(logicalName, wire, "result");
    const alias = scalarAlias(logicalName, "parse");
    this.#imports.add(
      `import { ${mapping.parse.name} as ${alias} } from ${JSON.stringify(mapping.parse.from)};`,
    );
    this.#usesParser = true;
    return `${alias} satisfies DsqlScalarParser<${wireType(wire, "result")}, ${hostType}>`;
  }

  imports(): string[] {
    return [...this.#imports].sort();
  }

  runtimeTypes(): string[] {
    return [
      ...(this.#usesParser ? ["DsqlScalarParser"] : []),
      ...(this.#usesSerializer ? ["DsqlScalarSerializer"] : []),
    ];
  }
}

type InputRoot =
  | typeof PARAMS_PREFIX
  | typeof INPUT_PREFIX
  | typeof CONTEXT_PREFIX;

export async function renderDsql(
  artifacts: BuildArtifacts,
  options: RenderDsqlOptions,
): Promise<DsqlRenderResult> {
  const scalars = options.scalars ?? {};
  validateDsqlScalarMappings(artifacts, scalars);
  const root = resolve(options.root);
  const queriesDir = resolveOutputDir(root, options.queriesDir);
  const executionDir = options.executionDir
    ? resolveOutputDir(root, options.executionDir)
    : undefined;
  const scope = options.scope ?? singleArtifactScope(artifacts);

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
    files.set(
      filePath,
      renderFragmentModule(artifacts, fragment, {
        embeddedSource: options.embeddedSources?.get(
          artifactKey("fragment", fragment.name),
        ),
        scalars,
      }),
    );
    queryExports.push(exportStatement(plan.fileStem));
    const id = artifacts.artifactIds.get(artifactKey("fragment", fragment.name));
    definitions[artifactKey("fragment", fragment.name)] = {
      name: fragment.name,
      kind: "fragment",
      ...(id ? { id } : {}),
      exportName: `${toPascalCase(fragment.name)}Fragment`,
      operationModule: moduleSpecifier(root, filePath),
      modulePath: projectRelativePath(root, filePath),
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
        outputMode: options.outputMode ?? "readable",
        embeddedSource: options.embeddedSources?.get(
          artifactKey("operation", operation.name),
        ),
        scalars,
      }),
    );
    queryExports.push(exportStatement(plan.fileStem));

    if (executionDir) {
      files.set(
        executionPath,
        renderOperationExecutionModule(operation, {
          operationImport: relativeModuleSpecifier(executionPath, operationPath),
          outputMode: options.outputMode ?? "readable",
          scalars,
        }),
      );
      executionExports.push(exportStatement(plan.fileStem));
    }

    const id = artifacts.artifactIds.get(artifactKey("operation", operation.name));
    definitions[artifactKey("operation", operation.name)] = {
      name: operation.name,
      kind: "query",
      ...(id ? { id } : {}),
      exportName: `${toPascalCase(operation.name)}Operation`,
      operationModule: moduleSpecifier(root, operationPath),
      modulePath: projectRelativePath(root, operationPath),
      executionModule: moduleSpecifier(root, executionPath),
    };
  }

  files.set(
    join(queriesDir, "index.ts"),
    renderBarrel(queryExports, { includeRuntime: true }),
  );
  if (executionDir) {
    files.set(join(executionDir, "index.ts"), renderBarrel(executionExports));
  }

  const renderedFiles = [...files.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([path, contents]) => ({ path, contents }));

  return {
    ...(scope
      ? {
          scope: {
            name: scope.name,
            imports: [...(scope.imports ?? [])],
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

function renderOperationModule(
  artifacts: BuildArtifacts,
  operation: OperationMetadata,
  options: {
    readonly includeExecutionPayload: boolean;
    readonly outputMode: DsqlOutputMode;
    readonly embeddedSource?: string | undefined;
    readonly scalars: DsqlScalarMappings;
  },
): string {
  const scalarContext = new ScalarModuleContext(options.scalars);
  const name = toPascalCase(operation.name);
  const manifestEntry = manifestEntryFor(artifacts, operation);
  const resultType = `${name}Result`;
  const wireResultType = `${name}WireResult`;
  const paramsType = `${name}Params`;
  const wireParamsType = `${name}WireParams`;
  const inputType = `${name}Input`;
  const wireInputType = `${name}WireInput`;
  const contextType = `${name}Context`;
  // Rendered before the imports: `used` collects exactly the fragment
  // result types the composition referenced.
  const resultCtx = resultTypeContext(
    operation.result.fields,
    operation.fragment_spreads,
    artifacts.fragments,
    scalarContext,
  );
  const resultLiteral = resultTypeLiteral(resultCtx);
  const wireResult = resultWireTypeLiteral(
    operation.result.fields,
    resultType,
    scalarContext,
  );
  const dynamicTypes = dynamicInputTypeDefinitions(operation, scalarContext);
  const paramsLiteral = paramsTypeLiteral(
    operation.params,
    scalarContext,
    operation.dynamic_inputs,
    operation.name,
  );
  const wireParams = paramsWireTypeLiteral(
    operation.params,
    paramsType,
    scalarContext,
    dynamicTypes.wireTypes,
  );
  const inputLiteral = inputTypeLiteral(
    operation.input,
    scalarContext,
    operation.fragment_spreads,
    artifacts.fragments,
  );
  const wireInput = inputWireTypeLiteral(
    operation.input,
    INPUT_PREFIX,
    inputType,
    scalarContext,
  );
  const contextLiteral = contextTypeLiteral(operation.context, scalarContext);
  const publicInputs = renderInputFields(
    [...operation.params, ...operation.input],
    scalarContext,
  );
  const dynamicInputContracts = renderDynamicInputContracts(
    operation.dynamic_inputs,
    scalarContext,
  );
  const resultMaterializer = options.includeExecutionPayload
    ? renderResultMaterializer(
        operation.result.fields,
        scalarContext,
        name,
        resultType,
        wireResultType,
      )
    : undefined;
  const executionPayload = options.includeExecutionPayload
    ? renderExecutionPayload(operation, `${name}Operation`, {
        exportedName: `${name}ExecutionPayload`,
        outputMode: options.outputMode,
        scalarContext,
        ...(resultMaterializer
          ? { resultMaterializer: resultMaterializer.name }
          : {}),
      })
    : undefined;
  const usesWireReplacement =
    wireResult.usesReplacement ||
    wireParams.usesReplacement ||
    wireInput.usesReplacement ||
    dynamicTypes.usesReplacement;
  const runtimeImports = [
    ...new Set([
      ...(options.includeExecutionPayload ? ["DsqlExecutionPayload"] : []),
      ...(resultUsesDatabaseArray(resultCtx) || wireResult.usesDatabaseArray
        ? ["DsqlDatabaseArray"]
        : []),
      ...(usesWireReplacement ? ["DsqlReplaceFields"] : []),
      "DsqlOperation",
      ...scalarContext.runtimeTypes(),
      "DsqlWireContract",
    ]),
  ].sort();
  const runtimeValues = resultMaterializer?.runtimeValues ?? [];
  const statements = [
    `import type { ${runtimeImports.join(", ")} } from "@dsql/typescript/runtime";`,
    ...(runtimeValues.length > 0
      ? [
          `import { ${runtimeValues.join(", ")} } from "@dsql/typescript/runtime";`,
        ]
      : []),
    ...scalarContext.imports(),
    ...fragmentTypeImports(resultCtx, operation),
    "",
    `export type ${resultType} = ${resultLiteral};`,
    "",
    `export type ${wireResultType} = ${wireResult.type};`,
    "",
    ...dynamicTypes.statements,
    `export type ${paramsType} = ${paramsLiteral};`,
    "",
    `export type ${wireParamsType} = ${wireParams.type};`,
    "",
    `export type ${inputType} = ${inputLiteral};`,
    "",
    `export type ${wireInputType} = ${wireInput.type};`,
    "",
    `export type ${contextType} = ${contextLiteral};`,
    "",
    `export const ${name}Operation: DsqlOperation<${resultType}, ${paramsType}, ${inputType}, ${contextType}, DsqlWireContract<${wireResultType}, ${wireParamsType}, ${wireInputType}>> = {
  id: ${JSON.stringify(manifestEntry.hash)},
  name: ${JSON.stringify(operation.name)},
  kind: ${JSON.stringify(DEFINITION_KIND_QUERY)},
  requiresContext: ${operation.context.length > 0},
  inputs: ${publicInputs},
  dynamicInputContracts: ${dynamicInputContracts}
};`,
    "",
    renderSourceRegistryAugmentation(options.embeddedSource, `${name}Operation`),
  ];

  if (executionPayload) {
    statements.push(
      "",
      ...(resultMaterializer ? [resultMaterializer.definition, ""] : []),
      executionPayload,
    );
  }

  return `${statements.join("\n")}\n`;
}

/**
 * Fragment-module type imports: result types come from the SAME
 * effective-spread set the composition used (never the raw spread list —
 * dropped transitive spreads must not leave unused imports); variables
 * types ride along for fragments the input envelope references.
 */
function fragmentTypeImports(
  ctx: ResultTypeCtx,
  operation?: OperationMetadata,
): string[] {
  const inputFragments = new Set(
    (operation?.input ?? []).flatMap((field) => inputPathFragmentNames(field.path)),
  );
  const names = new Set([...ctx.used, ...inputFragments]);
  const imports = [...names]
    .filter((name) => ctx.fragmentsByName.has(name))
    .map((name) => {
      const importedTypes: string[] = [];
      if (ctx.used.has(name)) {
        importedTypes.push(fragmentResultTypeName(name));
      }
      if (inputFragments.has(name)) {
        importedTypes.push(fragmentVariablesTypeName(name));
      }
      return `import type { ${importedTypes.join(", ")} } from ${JSON.stringify(
        `./${fragmentFileStem(name)}`,
      )};`;
    });
  imports.sort();
  return imports;
}

function inputPathFragmentNames(path: string): string[] {
  const parts = publicInputPath(path, INPUT_PREFIX);
  const names = [];
  for (let index = 0; index < parts.length - 1; index += 1) {
    const envelope = parts[index + 1];
    if (envelope === PARAMS_PREFIX || envelope === INPUT_PREFIX) {
      names.push(parts[index] ?? "");
    }
  }
  return names.filter(Boolean);
}

function renderOperationExecutionModule(
  operation: OperationMetadata,
  options: {
    readonly operationImport: string;
    readonly outputMode: DsqlOutputMode;
    readonly scalars: DsqlScalarMappings;
  },
): string {
  const scalarContext = new ScalarModuleContext(options.scalars);
  const name = toPascalCase(operation.name);
  const resultType = `${name}Result`;
  const wireResultType = `${name}WireResult`;
  const resultMaterializer = renderResultMaterializer(
    operation.result.fields,
    scalarContext,
    name,
    resultType,
    wireResultType,
  );
  const payload = renderExecutionPayload(operation, `${name}Operation`, {
    exportedName: `${name}ExecutionPayload`,
    outputMode: options.outputMode,
    scalarContext,
    ...(resultMaterializer
      ? { resultMaterializer: resultMaterializer.name }
      : {}),
  });
  const runtimeImports = [
    "DsqlExecutionPayload",
    ...scalarContext.runtimeTypes(),
  ].sort();
  const runtimeValues = resultMaterializer?.runtimeValues ?? [];
  return [
    `import type { ${runtimeImports.join(", ")} } from "@dsql/typescript/runtime";`,
    ...(runtimeValues.length > 0
      ? [
          `import { ${runtimeValues.join(", ")} } from "@dsql/typescript/runtime";`,
        ]
      : []),
    ...scalarContext.imports(),
    `import { ${name}Operation } from ${JSON.stringify(options.operationImport)};`,
    ...(resultMaterializer
      ? [
          `import type { ${resultType}, ${wireResultType} } from ${JSON.stringify(
            options.operationImport,
          )};`,
          "",
          resultMaterializer.definition,
        ]
      : []),
    "",
    payload,
    "",
  ].join("\n");
}

function renderExecutionPayload(
  operation: OperationMetadata,
  operationValue: string,
  options: {
    readonly exportedName: string;
    readonly outputMode: DsqlOutputMode;
    readonly scalarContext: ScalarModuleContext;
    readonly resultMaterializer?: string;
  },
): string {
  const sql =
    options.outputMode === "compact"
      ? JSON.stringify(operation.sql.compact_text)
      : typescriptTemplateLiteral(operation.sql.text);
  const materializer = options.resultMaterializer
    ? `  materializeResult: ${options.resultMaterializer},\n`
    : "";
  return `export const ${options.exportedName}: DsqlExecutionPayload<typeof ${operationValue}> = {
  operation: ${operationValue},
  parameters: ${JSON.stringify(operation.sql.parameters)},
  variants: ${JSON.stringify(sqlVariants(operation))},
  dynamicInputs: ${JSON.stringify(operation.dynamic_inputs)},
  contextInputs: ${renderInputFields(operation.context, options.scalarContext)},
${materializer}  sql: ${sql}
};`;
}

const TYPESCRIPT_PRINTER = ts.createPrinter({
  newLine: ts.NewLineKind.LineFeed,
});
const TYPESCRIPT_SOURCE_FILE = ts.createSourceFile(
  "generated.ts",
  "",
  ts.ScriptTarget.Latest,
  false,
  ts.ScriptKind.TS,
);

function typescriptTemplateLiteral(value: string): string {
  return TYPESCRIPT_PRINTER.printNode(
    ts.EmitHint.Expression,
    ts.factory.createNoSubstitutionTemplateLiteral(value),
    TYPESCRIPT_SOURCE_FILE,
  );
}

function renderFragmentModule(
  artifacts: BuildArtifacts,
  fragment: FragmentMetadata,
  options: {
    readonly embeddedSource?: string | undefined;
    readonly scalars: DsqlScalarMappings;
  },
): string {
  const scalarContext = new ScalarModuleContext(options.scalars);
  const name = toPascalCase(fragment.name);
  const resultType = fragmentResultTypeName(fragment.name);
  const paramsType = fragmentParamsTypeName(fragment.name);
  const inputType = fragmentInputTypeName(fragment.name);
  const variablesType = fragmentVariablesTypeName(fragment.name);
  // Fragments composed of other fragments reuse their types instead of
  // re-inlining: the body's spread provenance (empty path = fragment
  // root) drives the same composition operations use.
  const resultCtx = resultTypeContext(
    fragment.result.fields,
    fragment.fragment_spreads,
    artifacts.fragments,
    scalarContext,
  );
  const resultLiteral = resultTypeLiteral(resultCtx);
  const runtimeTypes = ["DsqlFragmentDefinition"];
  if (resultUsesDatabaseArray(resultCtx)) {
    runtimeTypes.unshift("DsqlDatabaseArray");
  }
  runtimeTypes.sort();
  const paramsLiteral = paramsTypeLiteral(
    fragment.params,
    scalarContext,
    [],
    fragment.name,
  );
  const inputLiteral = inputTypeLiteral(fragment.input, scalarContext);
  return [
    `import type { ${runtimeTypes.join(", ")} } from "@dsql/typescript/runtime";`,
    ...scalarContext.imports(),
    ...fragmentTypeImports(resultCtx),
    "",
    `export type ${resultType} = ${resultLiteral};`,
    "",
    `export type ${paramsType} = ${paramsLiteral};`,
    "",
    `export type ${inputType} = ${inputLiteral};`,
    "",
    `export type ${variablesType} = ${fragmentVariablesTypeLiteral(
      paramsType,
      inputType,
      fragment.params.some((field) => field.required),
      fragment.input.some((field) => field.required),
    )};`,
    "",
    `export const ${name}Fragment: DsqlFragmentDefinition<${resultType}, ${paramsType}, ${inputType}> = {
  name: ${JSON.stringify(fragment.name)},
  kind: ${JSON.stringify(DEFINITION_KIND_FRAGMENT)},
  table: ${JSON.stringify(fragment.table)}
};`,
    "",
    renderSourceRegistryAugmentation(options.embeddedSource, `${name}Fragment`),
    "",
  ].join("\n");
}

/**
 * Keys the source-string registry by exact embedded content. Only the
 * resolved definition of an expression gets a key, and only when the raw
 * bytes equal the cooked template-literal type
 * TypeScript infers: backslashes and `${` cook differently, and `\r`
 * is normalized out of cooked template lines — such content gets no
 * registry key rather than a wrong one.
 */
function renderSourceRegistryAugmentation(
  embeddedSource: string | undefined,
  valueName: string,
): string {
  if (
    embeddedSource === undefined ||
    embeddedSource.includes("\\") ||
    embeddedSource.includes("${") ||
    embeddedSource.includes("\r")
  ) {
    return "";
  }

  return `declare module "@dsql/typescript/runtime" {
  interface DsqlSourceRegistry {
    readonly ${JSON.stringify(embeddedSource)}: typeof ${valueName};
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
      `${name}WireResult`,
      `${name}Params`,
      `${name}WireParams`,
      `${name}Input`,
      `${name}WireInput`,
      ...operation.dynamic_inputs.flatMap((input) => [
        dynamicInputTypeName(operation.name, input.path, true),
        dynamicInputTypeName(operation.name, input.path, false),
      ]),
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
    const fileStem = fragmentFileStem(fragment.name);
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

function fragmentFileStem(name: string): string {
  return `${toPascalCase(name)}.fragment`;
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

function renderBarrel(
  exports: readonly string[],
  options: { readonly includeRuntime?: boolean } = {},
): string {
  const runtimeExports = options.includeRuntime
    ? [
        'export { dsql, dsqlQueryKey, dsqlQueryKeyForWire, materializeDsqlOperationVariables } from "@dsql/typescript/runtime";',
        'export type { DsqlDefinition, DsqlExecutionPayload, DsqlFragment, DsqlFragmentDefinition, DsqlFragmentInput, DsqlFragmentParams, DsqlFragmentVariables, DsqlMaterializedQuery, DsqlOperation, DsqlOperationContext, DsqlOperationInput, DsqlOperationParams, DsqlOperationResult, DsqlOperationWireInput, DsqlOperationWireParams, DsqlOperationWireResult, DsqlScalarParser, DsqlScalarSerializer, DsqlVariables, DsqlWireContract, DsqlWireVariables } from "@dsql/typescript/runtime";',
      ]
    : [];
  return `${[...runtimeExports, ...[...exports].sort()].join("\n")}\n`;
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

/** Project-base-relative file path with `/` separators (render map). */
function projectRelativePath(root: string, path: string): string {
  return normalizeModulePath(relative(root, path));
}

function singleArtifactScope(
  artifacts: BuildArtifacts,
): RenderDsqlOptions["scope"] | undefined {
  return artifacts.scopes.length === 1 ? artifacts.scopes[0] : undefined;
}

function sqlVariants(operation: OperationMetadata): Record<
  string,
  { readonly cases: Record<string, string>; readonly nullText?: string }
> {
  return Object.fromEntries(
    operation.sql.variants.map((variant) => [
      variant.path,
      {
        cases: Object.fromEntries(
          variant.cases.map((case_) => [case_.value, case_.text]),
        ),
        ...(variant.null_text === undefined
          ? {}
          : { nullText: variant.null_text }),
      },
    ]),
  );
}

/**
 * One definition's result-type composition state. Spread provenance is
 * path-qualified (the empty path is the definition root); `provided`
 * holds the ABSOLUTE result paths any spread's fragment contributes, so
 * the walk renders only the definition's own additions next to the
 * fragment result types it intersects. `used` collects the fragment
 * types the composition actually referenced — the import list must come
 * from the same effective-spread calculation as the types, or dropped
 * transitive spreads would leave unused imports (and nested ones missing
 * imports).
 */
type ResultTypeCtx = {
  readonly fields: readonly ResultField[];
  readonly spreads: readonly FragmentSpreadMetadata[];
  readonly fragmentsByName: ReadonlyMap<string, FragmentMetadata>;
  readonly provided: ReadonlySet<string>;
  readonly used: Set<string>;
  readonly scalars: ScalarModuleContext;
};

function resultTypeContext(
  fields: readonly ResultField[],
  spreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
  scalars: ScalarModuleContext,
): ResultTypeCtx {
  const fragmentsByName = new Map(fragments.map((fragment) => [fragment.name, fragment]));
  const provided = new Set<string>();
  for (const spread of spreads) {
    const fragment = fragmentsByName.get(spread.fragment);
    for (const field of fragment?.result.fields ?? []) {
      provided.add(spread.path === "" ? field.path : `${spread.path}.${field.path}`);
    }
  }
  return {
    fields,
    spreads,
    fragmentsByName,
    provided,
    used: new Set(),
    scalars,
  };
}

/** Adds `name` and every fragment its ROOT spreads reach (cycle-safe) —
 * root spreads land at the enclosing spread point, nested ones do not. */
function rootSpreadClosure(
  ctx: ResultTypeCtx,
  name: string,
  visited: Set<string>,
): void {
  if (visited.has(name)) {
    return;
  }
  visited.add(name);
  const fragment = ctx.fragmentsByName.get(name);
  for (const spread of fragment?.fragment_spreads ?? []) {
    if (spread.path === "") {
      rootSpreadClosure(ctx, spread.fragment, visited);
    }
  }
}

/**
 * Spreads landing at `path`, minus any that another spread at the SAME
 * path transitively provides through its root spreads (the plan walk
 * records transitively entered spreads too — intersecting both parent
 * and child fragment types would be redundant).
 */
function effectiveSpreadsAt(ctx: ResultTypeCtx, path: string): string[] {
  const direct: string[] = [];
  for (const spread of ctx.spreads) {
    if (spread.path === path && !direct.includes(spread.fragment)) {
      direct.push(spread.fragment);
    }
  }
  const covered = new Set<string>();
  for (const name of direct) {
    const closure = new Set<string>();
    rootSpreadClosure(ctx, name, closure);
    closure.delete(name);
    for (const inner of closure) {
      covered.add(inner);
    }
  }
  return direct.filter((name) => !covered.has(name));
}

/** Whether the field AND its whole subtree come from spreads — such
 * fields are dropped from the inline object (the intersected fragment
 * type carries them). A provided field with unprovided descendants
 * stays, restricted to those descendants by the recursive walk. */
function fullyProvided(ctx: ResultTypeCtx, field: ResultField): boolean {
  if (!ctx.provided.has(field.path)) {
    return false;
  }
  const prefix = `${field.path}.`;
  return ctx.fields.every(
    (candidate) => !candidate.path.startsWith(prefix) || ctx.provided.has(candidate.path),
  );
}

/** The object type at `path` ("" = definition root): effective spread
 * types intersected with the definition's own (non-provided) fields. */
function objectTypeAt(ctx: ResultTypeCtx, path: string): string {
  const own = ctx.fields.filter(
    (field) => field.parent_path === path && !fullyProvided(ctx, field),
  );
  const spreadTypes = effectiveSpreadsAt(ctx, path).map((name) => {
    ctx.used.add(name);
    return fragmentResultTypeName(name);
  });
  const ownType =
    own.length > 0 ? objectType(own.map((field) => propertyType(ctx, field))) : null;
  if (spreadTypes.length === 0) {
    return ownType ?? "Record<string, never>";
  }
  return ownType === null ? spreadTypes.join(" & ") : [...spreadTypes, ownType].join(" & ");
}

function resultTypeLiteral(ctx: ResultTypeCtx): string {
  return objectTypeAt(ctx, "");
}

type WireTypeExpression = {
  readonly type: string;
  readonly differs: boolean;
  readonly usesDatabaseArray: boolean;
  readonly usesReplacement: boolean;
};

function resultWireTypeLiteral(
  fields: readonly ResultField[],
  resultType: string,
  scalars: ScalarModuleContext,
): WireTypeExpression {
  const replacements = new Map<string, string>();
  let usesDatabaseArray = false;

  for (const field of fields) {
    if (field.kind !== RESULT_KIND_SCALAR) {
      continue;
    }
    if (
      hasClosedValues(field.value_type) ||
      !scalars.hasMapping(field.value_type.name)
    ) {
      continue;
    }
    const wireScalar = scalars.typeFor(
      field.value_type.name,
      field.value_type.wire.encoding,
      "result",
      false,
    );
    const wireValue =
      field.value_type.shape === RESULT_VALUE_SHAPE_DATABASE_ARRAY
        ? `DsqlDatabaseArray<${wireScalar}>`
        : wireScalar;
    replacements.set(field.path, withNullability(wireValue, field.nullable));
    usesDatabaseArray ||=
      field.value_type.shape === RESULT_VALUE_SHAPE_DATABASE_ARRAY;
  }

  if (replacements.size === 0) {
    return {
      type: resultType,
      differs: false,
      usesDatabaseArray: false,
      usesReplacement: false,
    };
  }

  return {
    type: resultWireObjectTypeAt(fields, replacements, "", resultType),
    differs: true,
    usesDatabaseArray,
    usesReplacement: true,
  };
}

function resultWireObjectTypeAt(
  fields: readonly ResultField[],
  replacements: ReadonlyMap<string, string>,
  path: string,
  baseType: string,
): string {
  const properties: Array<[string, string]> = [];
  for (const field of fields) {
    if (field.parent_path !== path) {
      continue;
    }
    const scalarReplacement = replacements.get(field.path);
    if (scalarReplacement !== undefined) {
      properties.push([field.name, scalarReplacement]);
      continue;
    }
    if (
      field.kind === RESULT_KIND_SCALAR ||
      ![...replacements.keys()].some((candidate) =>
        candidate.startsWith(`${field.path}.`),
      )
    ) {
      continue;
    }

    const propertyType = `${baseType}[${JSON.stringify(field.name)}]`;
    if (field.kind === RESULT_KIND_ARRAY) {
      const elementType = field.nullable
        ? `NonNullable<${propertyType}>[number]`
        : `${propertyType}[number]`;
      const elementWireType = resultWireObjectTypeAt(
        fields,
        replacements,
        field.path,
        elementType,
      );
      properties.push([
        field.name,
        withNullability(`Array<${elementWireType}>`, field.nullable),
      ]);
      continue;
    }

    const nestedWireType = resultWireObjectTypeAt(
      fields,
      replacements,
      field.path,
      field.nullable ? `NonNullable<${propertyType}>` : propertyType,
    );
    properties.push([
      field.name,
      withNullability(nestedWireType, field.nullable),
    ]);
  }

  return `DsqlReplaceFields<${baseType}, ${objectType(properties)}>`;
}

function paramsTypeLiteral(
  fields: readonly InputField[],
  scalars: ScalarModuleContext,
  dynamicInputs: readonly DynamicInputMetadata[] = [],
  operationName = "",
): string {
  return inputFieldsTypeLiteral(
    fields,
    PARAMS_PREFIX,
    scalars,
    [],
    [],
    new Map(
      dynamicInputs.map((input) => [
        input.path,
        dynamicInputTypeName(operationName, input.path, true),
      ]),
    ),
  );
}

function paramsWireTypeLiteral(
  fields: readonly InputField[],
  paramsType: string,
  scalars: ScalarModuleContext,
  dynamicWireTypes: ReadonlyMap<string, string>,
): WireTypeExpression {
  return inputWireTypeLiteral(
    fields,
    PARAMS_PREFIX,
    paramsType,
    scalars,
    dynamicWireTypes,
  );
}

function inputTypeLiteral(
  fields: readonly InputField[],
  scalars: ScalarModuleContext,
  fragmentSpreads: readonly FragmentSpreadMetadata[] = [],
  fragments: readonly FragmentMetadata[] = [],
): string {
  return inputFieldsTypeLiteral(
    fields,
    INPUT_PREFIX,
    scalars,
    fragmentSpreads,
    fragments,
    new Map(),
  );
}

function inputWireTypeLiteral(
  fields: readonly InputField[],
  prefix: InputRoot,
  hostType: string,
  scalars: ScalarModuleContext,
  dynamicWireTypes: ReadonlyMap<string, string> = new Map(),
): WireTypeExpression {
  const root = new WireOverlayNode();
  for (const field of fields) {
    const path = publicInputPath(field.path, prefix);
    if (path.length === 0) {
      continue;
    }

    const dynamicWireType = dynamicWireTypes.get(field.path);
    if (dynamicWireType !== undefined) {
      root.insert(path, dynamicWireType);
      continue;
    }
    if (SPECIAL_INPUT_TYPES.has(field.data_type)) {
      continue;
    }

    if (hasClosedValues(field) || !scalars.hasMapping(field.data_type)) {
      continue;
    }
    root.insert(path, inputFieldType(field, scalars, false));
  }

  if (root.isEmpty()) {
    return {
      type: hostType,
      differs: false,
      usesDatabaseArray: false,
      usesReplacement: false,
    };
  }
  return {
    type: root.toType(hostType),
    differs: true,
    usesDatabaseArray: false,
    usesReplacement: true,
  };
}

function contextTypeLiteral(
  fields: readonly InputField[],
  scalars: ScalarModuleContext,
): string {
  return inputFieldsTypeLiteral(
    fields,
    CONTEXT_PREFIX,
    scalars,
    [],
    [],
    new Map(),
  );
}

function inputFieldsTypeLiteral(
  fields: readonly InputField[],
  prefix: InputRoot,
  scalars: ScalarModuleContext,
  fragmentSpreads: readonly FragmentSpreadMetadata[] = [],
  fragments: readonly FragmentMetadata[] = [],
  dynamicTypes: ReadonlyMap<string, string> = new Map(),
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
    root.insert(branch.path, branch.type, branch.required);
  }

  for (const field of fields) {
    const path = publicInputPath(field.path, prefix);
    if (path.length === 0) {
      continue;
    }
    if (fragmentBranches.some((branch) => pathStartsWith(path, branch.path))) {
      continue;
    }
    root.insert(
      path,
      dynamicTypes.get(field.path) ?? inputFieldType(field, scalars, true),
      field.required,
    );
  }
  return root.toTypeLiteral();
}

type DynamicInputDefinitions = {
  readonly statements: readonly string[];
  readonly wireTypes: ReadonlyMap<string, string>;
  readonly usesReplacement: boolean;
};

function dynamicInputTypeDefinitions(
  operation: OperationMetadata,
  scalars: ScalarModuleContext,
): DynamicInputDefinitions {
  if (operation.dynamic_inputs.length === 0) {
    return {
      statements: [],
      wireTypes: new Map(),
      usesReplacement: false,
    };
  }
  const statements: string[] = [];
  const wireTypes = new Map<string, string>();
  let usesReplacement = false;
  for (const input of operation.dynamic_inputs) {
    const hostName = dynamicInputTypeName(operation.name, input.path, true);
    const wireName = dynamicInputTypeName(operation.name, input.path, false);
    const hostLiteral = dynamicInputTypeLiteral(
      operation.name,
      input,
      scalars,
    );
    const wire = dynamicInputWireTypeLiteral(operation.name, input, scalars);
    if (wire.differs) {
      wireTypes.set(input.path, wireName);
    }
    usesReplacement ||= wire.usesReplacement;
    statements.push(
      `export type ${hostName} = ${hostLiteral};`,
      "",
      `export type ${wireName} = ${wire.type};`,
      "",
    );
  }
  return { statements, wireTypes, usesReplacement };
}

function dynamicInputTypeName(
  operationName: string,
  path: string,
  mapped: boolean,
): string {
  return `${toPascalCase(operationName)}${path
    .split(".")
    .map(toPascalCase)
    .join("")}${mapped ? "" : "Wire"}DynamicInput`;
}

function dynamicInputTypeLiteral(
  operationName: string,
  input: DynamicInputMetadata,
  scalars: ScalarModuleContext,
): string {
  const typeName = dynamicInputTypeName(operationName, input.path, true);
  if (input.kind === "order") {
    const entries = input.fields.map((field) =>
      objectType([
        [
          field.key,
          field.directions
            .map((direction) => JSON.stringify(direction))
            .join(" | "),
        ],
      ]),
    );
    return `Array<${entries.length === 0 ? "never" : entries.join(" | ")}>`;
  }
  const properties: Array<{
    readonly name: string;
    readonly type: string;
    readonly optional: boolean;
  }> = [
    { name: "and", type: `Array<${typeName}>`, optional: true },
    { name: "or", type: `Array<${typeName}>`, optional: true },
    { name: "not", type: typeName, optional: true },
  ];
  const operatorKinds = dynamicOperatorKinds(input);
  for (const field of input.fields) {
    const scalar = hasClosedValues(field)
      ? closedValueType(field)
      : scalars.typeFor(
          field.data_type,
          field.wire.encoding,
          "input",
          true,
        );
    properties.push({
      name: field.key,
      type: objectTypeWithOptional(
        field.operators.map((operator) => ({
          name: operator,
          type: dynamicOperatorType(
            operatorKinds,
            input.path,
            field.key,
            operator,
            scalar,
          ),
          optional: true,
        })),
      ),
      optional: true,
    });
  }
  return objectTypeWithOptional(properties);
}

function dynamicInputWireTypeLiteral(
  operationName: string,
  input: DynamicInputMetadata,
  scalars: ScalarModuleContext,
): WireTypeExpression {
  const hostName = dynamicInputTypeName(operationName, input.path, true);
  const wireName = dynamicInputTypeName(operationName, input.path, false);
  if (input.kind === "order") {
    return {
      type: hostName,
      differs: false,
      usesDatabaseArray: false,
      usesReplacement: false,
    };
  }

  const operatorKinds = dynamicOperatorKinds(input);
  const properties: Array<{
    readonly name: string;
    readonly type: string;
    readonly optional: boolean;
  }> = [];
  for (const field of input.fields) {
    if (hasClosedValues(field) || !scalars.hasMapping(field.data_type)) {
      continue;
    }
    const wireScalar = scalars.typeFor(
      field.data_type,
      field.wire.encoding,
      "input",
      false,
    );
    const operators = field.operators
      .filter(
        (operator) =>
          requiredDynamicOperatorKind(
            operatorKinds,
            input.path,
            field.key,
            operator,
          ) !== "boolean",
      )
      .map((operator) => ({
        name: operator,
        type: dynamicOperatorType(
          operatorKinds,
          input.path,
          field.key,
          operator,
          wireScalar,
        ),
        optional: false,
      }));
    if (operators.length === 0) {
      continue;
    }
    properties.push({
      name: field.key,
      type: `DsqlReplaceFields<NonNullable<${hostName}[${JSON.stringify(
        field.key,
      )}]>, ${objectTypeWithOptional(operators)}>`,
      optional: false,
    });
  }

  if (properties.length === 0) {
    return {
      type: hostName,
      differs: false,
      usesDatabaseArray: false,
      usesReplacement: false,
    };
  }
  return {
    type: `DsqlReplaceFields<${hostName}, ${objectTypeWithOptional([
      { name: "and", type: `Array<${wireName}>`, optional: false },
      { name: "or", type: `Array<${wireName}>`, optional: false },
      { name: "not", type: wireName, optional: false },
      ...properties,
    ])}>`,
    differs: true,
    usesDatabaseArray: false,
    usesReplacement: true,
  };
}

type DynamicOperatorValueKind = "scalar" | "collection" | "boolean";

function dynamicOperatorKinds(
  input: DynamicInputMetadata,
): ReadonlyMap<string, ReadonlyMap<string, DynamicOperatorValueKind>> {
  const fields = new Map(
    input.fields.map((field) => [
      field.key,
      new Map<string, DynamicOperatorValueKind>(),
    ]),
  );
  if (input.kind === "order") {
    return fields;
  }
  if (input.sites.length === 0) {
    throw new Error(
      `dsql dynamic input ${input.path} has no generated SQL usage sites`,
    );
  }

  for (const site of input.sites) {
    for (const siteField of site.fields) {
      const declared = input.fields.find(
        (field) => field.key === siteField.key,
      );
      const kinds = fields.get(siteField.key);
      if (!declared || !kinds) {
        throw new Error(
          `dsql dynamic input ${input.path} site ${site.marker} has unknown field ${siteField.key}`,
        );
      }
      for (const operator of siteField.operators) {
        if (!declared.operators.includes(operator.name)) {
          throw new Error(
            `dsql dynamic input ${input.path}.${siteField.key} site ${site.marker} has undeclared operator ${operator.name}`,
          );
        }
        const kind = dynamicOperatorValueKind(
          input.path,
          siteField.key,
          operator.name,
          operator.value_kind,
        );
        const previous = kinds.get(operator.name);
        if (previous !== undefined && previous !== kind) {
          throw new Error(
            `dsql dynamic input ${input.path}.${siteField.key}.${operator.name} disagrees on value kind across SQL sites`,
          );
        }
        kinds.set(operator.name, kind);
      }
    }
  }

  for (const field of input.fields) {
    const kinds = fields.get(field.key);
    for (const operator of field.operators) {
      if (!kinds?.has(operator)) {
        throw new Error(
          `dsql dynamic input ${input.path}.${field.key}.${operator} has no generated SQL operator metadata`,
        );
      }
    }
  }
  return fields;
}

function dynamicOperatorValueKind(
  inputPath: string,
  field: string,
  operator: string,
  value: string,
): DynamicOperatorValueKind {
  if (value === "scalar" || value === "collection" || value === "boolean") {
    return value;
  }
  throw new Error(
    `dsql dynamic input ${inputPath}.${field}.${operator} has unknown value kind ${value}`,
  );
}

function dynamicOperatorType(
  kinds: ReadonlyMap<
    string,
    ReadonlyMap<string, DynamicOperatorValueKind>
  >,
  inputPath: string,
  field: string,
  operator: string,
  scalar: string,
): string {
  const kind = requiredDynamicOperatorKind(
    kinds,
    inputPath,
    field,
    operator,
  );
  if (kind === "boolean") {
    return "boolean";
  }
  if (kind === "collection") {
    return `Array<${scalar}>`;
  }
  if (kind === "scalar") {
    return scalar;
  }
  throw new Error(
    `dsql dynamic input ${inputPath}.${field}.${operator} has unsupported value kind ${kind}`,
  );
}

function requiredDynamicOperatorKind(
  kinds: ReadonlyMap<
    string,
    ReadonlyMap<string, DynamicOperatorValueKind>
  >,
  inputPath: string,
  field: string,
  operator: string,
): DynamicOperatorValueKind {
  const kind = kinds.get(field)?.get(operator);
  if (kind !== undefined) {
    return kind;
  }
  throw new Error(
    `dsql dynamic input ${inputPath}.${field}.${operator} has no value kind`,
  );
}

function operationFragmentInputBranches(
  fields: readonly InputField[],
  fragmentSpreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): Array<{
  readonly path: readonly string[];
  readonly type: string;
  readonly required: boolean;
}> {
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
      required: boolean;
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
        required: false,
      };
      branch.hasParams ||= envelope === PARAMS_PREFIX;
      branch.hasInput ||= envelope === INPUT_PREFIX;
      branch.required ||= field.required;
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
      required: branch.required,
    }));
}

function inputFieldType(
  field: InputField,
  scalars: ScalarModuleContext,
  mapped: boolean,
): string {
  const elementType =
    hasClosedValues(field)
      ? closedValueType(field)
      : scalars.typeFor(
          field.data_type,
          field.wire.encoding,
          "input",
          mapped,
        );
  const type = field.collection === true ? `Array<${elementType} | null>` : elementType;
  return withNullability(type, field.nullable);
}

function publicInputPath(path: string, prefix: InputRoot): string[] {
  const parts = path.split(".").filter(Boolean);
  if (parts[0] !== prefix) {
    return parts;
  }
  return parts.slice(1);
}

function propertyType(ctx: ResultTypeCtx, field: ResultField): [string, string] {
  if (field.kind === RESULT_KIND_SCALAR) {
    const scalar = hasClosedValues(field.value_type)
      ? closedValueType(field.value_type)
      : ctx.scalars.typeFor(
          field.value_type.name,
          field.value_type.wire.encoding,
          "result",
          true,
        );
    const value =
      field.value_type.shape === RESULT_VALUE_SHAPE_DATABASE_ARRAY
        ? `DsqlDatabaseArray<${scalar}>`
        : scalar;
    return [
      field.name,
      withNullability(value, field.nullable),
    ];
  }
  const type = objectTypeAt(ctx, field.path);
  const resultType = field.kind === RESULT_KIND_ARRAY ? `Array<${type}>` : type;
  return [field.name, withNullability(resultType, field.nullable)];
}

function resultUsesDatabaseArray(ctx: ResultTypeCtx): boolean {
  return ctx.fields.some(
    (field) =>
      !fullyProvided(ctx, field) &&
      field.value_type.shape === RESULT_VALUE_SHAPE_DATABASE_ARRAY,
  );
}

function pathStartsWith(path: readonly string[], prefix: readonly string[]): boolean {
  return prefix.every((part, index) => path[index] === part);
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
  private valueRequired = false;

  insert(path: readonly string[], type: string, required = true): void {
    if (path.length === 0) {
      this.value = type;
      this.valueRequired ||= required;
      return;
    }

    const [head, ...tail] = path;
    if (head === undefined) {
      this.value = type;
      this.valueRequired ||= required;
      return;
    }
    let child = this.children.get(head);
    if (!child) {
      child = new TypeNode();
      this.children.set(head, child);
    }
    child.insert(tail, type, required);
  }

  toTypeLiteral(): string {
    if (this.children.size === 0) {
      return this.value ?? UNKNOWN_TS_TYPE;
    }

    return objectTypeWithOptional(
      [...this.children.entries()].map(([name, child]) => ({
        name,
        type: child.toTypeLiteral(),
        optional: !child.isRequired(),
      })),
    );
  }

  private isRequired(): boolean {
    return this.valueRequired || [...this.children.values()].some((child) => child.isRequired());
  }
}

class WireOverlayNode {
  private readonly children = new Map<string, WireOverlayNode>();
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
      child = new WireOverlayNode();
      this.children.set(head, child);
    }
    child.insert(tail, type);
  }

  isEmpty(): boolean {
    return this.value === undefined && this.children.size === 0;
  }

  toType(baseType: string): string {
    if (this.value !== undefined) {
      return this.value;
    }
    return `DsqlReplaceFields<${baseType}, ${objectTypeWithOptional(
      [...this.children.entries()].map(([name, child]) => ({
        name,
        type: child.toType(
          `NonNullable<${baseType}[${JSON.stringify(name)}]>`,
        ),
        optional: false,
      })),
    )}>`;
  }
}

function propertyName(name: string): string {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? name : JSON.stringify(name);
}

function withNullability(type: string, nullable: boolean): string {
  return nullable ? `${type} | null` : type;
}

function wireType(wire: WireEncoding, direction: ScalarDirection): string {
  switch (wire) {
    case "text":
    case "text_cast":
    case "uuid":
    case "timestamptz":
    case "big_integer":
    case "numeric":
      return "string";
    case "integer":
      return "number";
    case "float":
      return direction === "result" ? FLOAT_TS_TYPE : "number";
    case "boolean":
      return "boolean";
    case "json":
    case "unsupported":
      return UNKNOWN_TS_TYPE;
  }
}

function scalarAlias(logicalName: string, role: ScalarRole): string {
  const identity = [...logicalName]
    .map((character) => character.codePointAt(0)?.toString(16) ?? "0")
    .join("_");
  return `__dsql_scalar_${identity}_${role}`;
}

function renderInputFields(
  fields: readonly InputField[],
  scalars: ScalarModuleContext,
): string {
  return `[${fields
    .map((field) => {
      const serializer =
        !hasClosedValues(field)
          ? scalars.serializerFor(
              field.data_type,
              field.wire.encoding,
            )
          : undefined;
      return renderRuntimeValue(field, "serialize", serializer);
    })
    .join(",")}]`;
}

function renderDynamicInputContracts(
  inputs: readonly DynamicInputMetadata[],
  scalars: ScalarModuleContext,
): string {
  const contracts = inputs.map((input) => {
    const operatorKinds = dynamicOperatorKinds(input);
    return {
      path: input.path,
      kind: input.kind,
      fields: input.fields.map((field) => {
        const value = {
          key: field.key,
          data_type: field.data_type,
          wire: field.wire,
          validation: field.validation,
          closed_values: field.closed_values,
          operators: field.operators.map((operator) => ({
            name: operator,
            value_kind: requiredDynamicOperatorKind(
              operatorKinds,
              input.path,
              field.key,
              operator,
            ),
          })),
        };
        return {
          value,
          serializer:
            input.kind === "predicate" && !hasClosedValues(field)
              ? scalars.serializerFor(
                  field.data_type,
                  field.wire.encoding,
                )
              : undefined,
        };
      }),
    };
  });
  return `[${contracts
    .map(
      (input) =>
        `{"path":${JSON.stringify(input.path)},"kind":${JSON.stringify(
          input.kind,
        )},"fields":[${input.fields
          .map((field) =>
            renderRuntimeValue(field.value, "serialize", field.serializer),
          )
          .join(",")}]}`,
    )
    .join(",")}]`;
}

type RenderedResultMaterializer = {
  readonly name: string;
  readonly definition: string;
  readonly runtimeValues: readonly string[];
};

type ResultMaterializerState = {
  readonly children: ReadonlyMap<string, readonly ResultField[]>;
  readonly parsers: ReadonlyMap<string, string>;
  readonly visitedPaths: ReadonlySet<string>;
  nextLocal: number;
  usesDatabaseArray: boolean;
  usesScalar: boolean;
};

/**
 * Emits direct traversal only for result branches that contain a configured
 * scalar parser. Operation result metadata is fully expanded, including
 * fragment-provided fields, so fragments need no independent materializer.
 */
function renderResultMaterializer(
  fields: readonly ResultField[],
  scalars: ScalarModuleContext,
  operationName: string,
  resultType: string,
  wireResultType: string,
): RenderedResultMaterializer | undefined {
  const parsers = new Map<string, string>();
  const fieldsByPath = new Map(fields.map((field) => [field.path, field]));
  for (const field of fields) {
    if (field.kind !== RESULT_KIND_SCALAR) {
      continue;
    }
    if (hasClosedValues(field.value_type)) {
      continue;
    }
    const parser = scalars.parserFor(
      field.value_type.name,
      field.value_type.wire.encoding,
    );
    if (parser) {
      parsers.set(field.path, parser);
    }
  }
  if (parsers.size === 0) {
    return undefined;
  }

  const visitedPaths = new Set<string>();
  for (const path of parsers.keys()) {
    let current = fieldsByPath.get(path);
    while (current) {
      visitedPaths.add(current.path);
      if (current.parent_path === "") {
        break;
      }
      const parent = fieldsByPath.get(current.parent_path);
      if (!parent) {
        throw new Error(
          `dsql result parser path ${path} has no parent field ${current.parent_path}`,
        );
      }
      current = parent;
    }
  }

  const children = new Map<string, ResultField[]>();
  for (const field of fields) {
    const group = children.get(field.parent_path) ?? [];
    group.push(field);
    children.set(field.parent_path, group);
  }
  const state: ResultMaterializerState = {
    children,
    parsers,
    visitedPaths,
    nextLocal: 0,
    usesDatabaseArray: false,
    usesScalar: false,
  };
  const body = renderResultMaterializerChildren(state, "", "result", "  ");
  const name = `materialize${operationName}Result`;
  return {
    name,
    definition: `function ${name}(
  result: ${wireResultType},
): ${resultType} {
${body.join("\n")}
  return result as unknown as ${resultType};
}`,
    runtimeValues: [
      "assignDsqlResultField",
      ...(state.usesDatabaseArray
        ? ["materializeDsqlDatabaseArrayResult"]
        : []),
      ...(state.usesScalar ? ["materializeDsqlScalarResult"] : []),
    ],
  };
}

function renderResultMaterializerChildren(
  state: ResultMaterializerState,
  parentPath: string,
  parentValue: string,
  indent: string,
): string[] {
  return (state.children.get(parentPath) ?? []).flatMap((field) =>
    state.visitedPaths.has(field.path)
      ? renderResultMaterializerField(state, field, parentValue, indent)
      : [],
  );
}

function renderResultMaterializerField(
  state: ResultMaterializerState,
  field: ResultField,
  parentValue: string,
  indent: string,
): string[] {
  const value = `_dsqlResult${state.nextLocal}`;
  state.nextLocal += 1;
  const lines = [
    `${indent}const ${value} = ${parentValue}[${JSON.stringify(field.name)}];`,
  ];

  if (field.kind === RESULT_KIND_SCALAR) {
    const parser = state.parsers.get(field.path);
    if (!parser) {
      return lines;
    }
    const materialized =
      field.value_type.shape === RESULT_VALUE_SHAPE_DATABASE_ARRAY
        ? materializeDatabaseArrayExpression(state, field, value, parser)
        : materializeScalarExpression(state, field, value, parser);
    if (field.nullable) {
      lines.push(
        `${indent}if (${value} !== null) {`,
        `${indent}  assignDsqlResultField(${parentValue}, ${JSON.stringify(
          field.name,
        )}, ${materialized});`,
        `${indent}}`,
      );
    } else {
      lines.push(
        `${indent}if (${value} === null) {`,
        `${indent}  throw new Error(${JSON.stringify(
          `non-null dsql result is null at ${field.path}`,
        )});`,
        `${indent}}`,
        `${indent}assignDsqlResultField(${parentValue}, ${JSON.stringify(
          field.name,
        )}, ${materialized});`,
      );
    }
    return lines;
  }

  const nestedIndent = field.nullable ? `${indent}  ` : indent;
  if (field.nullable) {
    lines.push(`${indent}if (${value} !== null) {`);
  } else {
    lines.push(
      `${indent}if (${value} === null) {`,
      `${indent}  throw new Error(${JSON.stringify(
        `non-null dsql result is null at ${field.path}`,
      )});`,
      `${indent}}`,
    );
  }

  if (field.kind === RESULT_KIND_ARRAY) {
    const item = `_dsqlResult${state.nextLocal}`;
    state.nextLocal += 1;
    lines.push(
      `${nestedIndent}if (!Array.isArray(${value})) {`,
      `${nestedIndent}  throw new Error(${JSON.stringify(
        `dsql result ${field.path} must be an array`,
      )});`,
      `${nestedIndent}}`,
      `${nestedIndent}for (const ${item} of ${value}) {`,
      `${nestedIndent}  if (typeof ${item} !== "object" || ${item} === null || Array.isArray(${item})) {`,
      `${nestedIndent}    throw new Error(${JSON.stringify(
        `dsql result ${field.path} must contain objects`,
      )});`,
      `${nestedIndent}  }`,
      ...renderResultMaterializerChildren(
        state,
        field.path,
        item,
        `${nestedIndent}  `,
      ),
      `${nestedIndent}}`,
    );
  } else {
    lines.push(
      `${nestedIndent}if (typeof ${value} !== "object" || Array.isArray(${value})) {`,
      `${nestedIndent}  throw new Error(${JSON.stringify(
        `dsql result ${field.path} must be an object`,
      )});`,
      `${nestedIndent}}`,
      ...renderResultMaterializerChildren(
        state,
        field.path,
        value,
        nestedIndent,
      ),
    );
  }
  if (field.nullable) {
    lines.push(`${indent}}`);
  }
  return lines;
}

function materializeDatabaseArrayExpression(
  state: ResultMaterializerState,
  field: ResultField,
  value: string,
  parser: string,
): string {
  state.usesDatabaseArray = true;
  return `materializeDsqlDatabaseArrayResult(${value}, ${parser}, ${JSON.stringify(
    field.path,
  )}, ${JSON.stringify(field.value_type.name)})`;
}

function materializeScalarExpression(
  state: ResultMaterializerState,
  field: ResultField,
  value: string,
  parser: string,
): string {
  state.usesScalar = true;
  return `materializeDsqlScalarResult(${value}, ${parser}, ${JSON.stringify(
    field.path,
  )}, ${JSON.stringify(field.value_type.name)})`;
}

function renderRuntimeValue(
  value: unknown,
  property: "parse" | "serialize",
  expression: string | undefined,
): string {
  const serialized = JSON.stringify(value);
  return expression === undefined
    ? serialized
    : `{...${serialized},${property}:${expression}}`;
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
