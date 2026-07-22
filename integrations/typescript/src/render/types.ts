import {
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import type {
  FragmentMetadata,
  FragmentSpreadMetadata,
  InputField,
  OperationManifestEntry,
  OperationMetadata,
  ResultField,
} from "../generated/metadata.ts";
import type { BuildArtifacts } from "../node.ts";

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
};

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
const PARAMS_PREFIX = "params";
const INPUT_PREFIX = "input";
const CONTEXT_PREFIX = "context";
const FLOAT_TS_TYPE = 'number | "NaN" | "Infinity" | "-Infinity"';
const UNKNOWN_TS_TYPE = "unknown";
const RENDER_DSQL_VERSION = 1;
const RENDER_MANIFEST_NAME = ".dsql-render-manifest.json";

type InputRoot =
  | typeof PARAMS_PREFIX
  | typeof INPUT_PREFIX
  | typeof CONTEXT_PREFIX;

export async function renderDsql(
  artifacts: BuildArtifacts,
  options: RenderDsqlOptions,
): Promise<DsqlRenderResult> {
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
        embeddedSource: options.embeddedSources?.get(
          artifactKey("operation", operation.name),
        ),
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

  const manifestFile = writeRenderedFiles({
    manifestPath: join(queriesDir, RENDER_MANIFEST_NAME),
    files: renderedFiles,
    artifactHash: artifactHash(artifacts),
    ...(scope?.name ? { scopeName: scope.name } : {}),
    layout: {
      queriesDir,
      ...(executionDir ? { executionDir } : {}),
    },
  });

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
    // The ownership manifest is a written file too: render maps must
    // account for every write inside an owned root.
    files: [...renderedFiles, manifestFile],
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
    readonly embeddedSource?: string | undefined;
  },
): string {
  const name = toPascalCase(operation.name);
  const manifestEntry = manifestEntryFor(artifacts, operation);
  const resultType = `${name}Result`;
  const paramsType = `${name}Params`;
  const inputType = `${name}Input`;
  const contextType = `${name}Context`;
  const runtimeImports = ["DsqlOperation"];
  if (options.includeExecutionPayload) {
    runtimeImports.unshift("DsqlExecutionPayload");
  }
  // Rendered before the imports: `used` collects exactly the fragment
  // result types the composition referenced.
  const resultCtx = resultTypeContext(
    operation.result.fields,
    operation.fragment_spreads,
    artifacts.fragments,
  );
  const resultLiteral = resultTypeLiteral(resultCtx);
  const statements = [
    `import type { ${runtimeImports.join(", ")} } from "@dsql/typescript/runtime";`,
    ...fragmentTypeImports(resultCtx, operation),
    "",
    `export type ${resultType} = ${resultLiteral};`,
    "",
    `export type ${paramsType} = ${paramsTypeLiteral(operation.params)};`,
    "",
    `export type ${inputType} = ${inputTypeLiteral(
      operation.input,
      operation.fragment_spreads,
      artifacts.fragments,
    )};`,
    "",
    `export type ${contextType} = ${contextTypeLiteral(operation.context)};`,
    "",
    `export const ${name}Operation: DsqlOperation<${resultType}, ${paramsType}, ${inputType}, ${contextType}> = {
  id: ${JSON.stringify(manifestEntry.hash)},
  name: ${JSON.stringify(operation.name)},
  kind: ${JSON.stringify(DEFINITION_KIND_QUERY)},
  requiresContext: ${operation.context.length > 0},
  inputs: ${JSON.stringify([...operation.params, ...operation.input])}
};`,
    "",
    renderSourceRegistryAugmentation(options.embeddedSource, `${name}Operation`),
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
  variants: ${JSON.stringify(sqlVariants(operation))},
  inputs: ${JSON.stringify([...operation.params, ...operation.input, ...operation.context])}
};`;
}

function renderFragmentModule(
  artifacts: BuildArtifacts,
  fragment: FragmentMetadata,
  options: {
    readonly embeddedSource?: string | undefined;
  },
): string {
  const name = toPascalCase(fragment.name);
  const resultType = fragmentResultTypeName(fragment.name);
  const paramsType = fragmentParamsTypeName(fragment.name);
  const inputType = fragmentInputTypeName(fragment.name);
  const variablesType = fragmentVariablesTypeName(fragment.name);
  // Fragments composed of other fragments reuse their types instead of
  // re-inlining: the body's spread provenance (empty path = fragment
  // root) drives the same composition operations use. Artifacts written
  // before the field existed degrade to the inline shape.
  const resultCtx = resultTypeContext(
    fragment.result.fields,
    fragment.fragment_spreads ?? [],
    artifacts.fragments,
  );
  const resultLiteral = resultTypeLiteral(resultCtx);
  return [
    `import type { DsqlFragmentDefinition } from "@dsql/typescript/runtime";`,
    ...fragmentTypeImports(resultCtx),
    "",
    `export type ${resultType} = ${resultLiteral};`,
    "",
    `export type ${paramsType} = ${paramsTypeLiteral(fragment.params)};`,
    "",
    `export type ${inputType} = ${inputTypeLiteral(fragment.input)};`,
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
        'export { dsql } from "@dsql/typescript/runtime";',
        'export type { DsqlDefinition, DsqlExecutionPayload, DsqlFragment, DsqlFragmentDefinition, DsqlFragmentInput, DsqlFragmentParams, DsqlFragmentVariables, DsqlMaterializedQuery, DsqlOperation, DsqlOperationContext, DsqlOperationInput, DsqlOperationParams, DsqlOperationResult, DsqlVariables } from "@dsql/typescript/runtime";',
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

type RenderOwnershipManifest = {
  readonly version: typeof RENDER_DSQL_VERSION;
  readonly renderer: "renderDsql";
  readonly rendererVersion: typeof RENDER_DSQL_VERSION;
  readonly artifactHash: string;
  readonly scopeName?: string;
  readonly layoutHash: string;
  readonly files: readonly {
    readonly path: string;
    readonly contentHash: string;
  }[];
};

function writeRenderedFiles(options: {
  readonly manifestPath: string;
  readonly files: readonly DsqlRenderedFile[];
  readonly artifactHash: string;
  readonly scopeName?: string;
  readonly layout: {
    readonly queriesDir: string;
    readonly executionDir?: string;
  };
}): DsqlRenderedFile {
  const manifest = renderOwnershipManifest(options);
  const previous = readRenderOwnershipManifest(options.manifestPath);
  const nextPaths = new Set(options.files.map((file) => file.path));

  for (const stale of previous?.files ?? []) {
    if (nextPaths.has(stale.path) || stale.path === options.manifestPath) {
      continue;
    }
    if (existsSync(stale.path)) {
      unlinkSync(stale.path);
    }
  }

  for (const file of options.files) {
    writeFileIfChanged(file.path, file.contents);
  }
  const contents = `${JSON.stringify(manifest, null, 2)}\n`;
  writeFileIfChanged(options.manifestPath, contents);
  return { path: options.manifestPath, contents };
}

function renderOwnershipManifest(options: {
  readonly files: readonly DsqlRenderedFile[];
  readonly artifactHash: string;
  readonly scopeName?: string;
  readonly layout: {
    readonly queriesDir: string;
    readonly executionDir?: string;
  };
}): RenderOwnershipManifest {
  return {
    version: RENDER_DSQL_VERSION,
    renderer: "renderDsql",
    rendererVersion: RENDER_DSQL_VERSION,
    artifactHash: options.artifactHash,
    ...(options.scopeName ? { scopeName: options.scopeName } : {}),
    layoutHash: hashJson({
      scopeName: options.scopeName,
      layout: options.layout,
      rendererVersion: RENDER_DSQL_VERSION,
    }),
    files: options.files.map((file) => ({
      path: file.path,
      contentHash: hashString(file.contents),
    })),
  };
}

function readRenderOwnershipManifest(
  manifestPath: string,
): RenderOwnershipManifest | undefined {
  if (!existsSync(manifestPath)) {
    return undefined;
  }

  try {
    const manifest = JSON.parse(
      readFileSync(manifestPath, "utf8"),
    ) as RenderOwnershipManifest;
    if (manifest.renderer !== "renderDsql") {
      return undefined;
    }
    return manifest;
  } catch {
    return undefined;
  }
}

function writeFileIfChanged(path: string, contents: string): void {
  if (existsSync(path) && readFileSync(path, "utf8") === contents) {
    return;
  }
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
}

function artifactHash(artifacts: BuildArtifacts): string {
  return hashJson({
    manifest: artifacts.manifest,
    operations: artifacts.manifest.operations.map((operation) => ({
      name: operation.name,
      hash: operation.hash,
    })),
    fragments: artifacts.manifest.fragments.map((fragment) => ({
      name: fragment.name,
      hash: fragment.hash,
    })),
  });
}

function hashJson(value: unknown): string {
  return hashString(JSON.stringify(value));
}

function hashString(value: string): string {
  return createHash("sha256").update(value).digest("hex");
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
};

function resultTypeContext(
  fields: readonly ResultField[],
  spreads: readonly FragmentSpreadMetadata[],
  fragments: readonly FragmentMetadata[],
): ResultTypeCtx {
  const fragmentsByName = new Map(fragments.map((fragment) => [fragment.name, fragment]));
  const provided = new Set<string>();
  for (const spread of spreads) {
    const fragment = fragmentsByName.get(spread.fragment);
    for (const field of fragment?.result.fields ?? []) {
      provided.add(spread.path === "" ? field.path : `${spread.path}.${field.path}`);
    }
  }
  return { fields, spreads, fragmentsByName, provided, used: new Set() };
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

function contextTypeLiteral(fields: readonly InputField[]): string {
  return inputFieldsTypeLiteral(fields, CONTEXT_PREFIX);
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
    root.insert(path, inputFieldType(field), field.required);
  }
  return root.toTypeLiteral();
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

function inputFieldType(field: InputField): string {
  const elementType =
    field.enum_values.length > 0
      ? field.enum_values.map((value) => JSON.stringify(value)).join(" | ")
      : dataType(field.data_type);
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
    return [
      field.name,
      withNullability(dataType(field.data_type), field.nullable),
    ];
  }
  const type = objectTypeAt(ctx, field.path);
  const resultType = field.kind === RESULT_KIND_ARRAY ? `Array<${type}>` : type;
  return [field.name, withNullability(resultType, field.nullable)];
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
    case "float":
      return FLOAT_TS_TYPE;
    case "numeric":
      return "string";
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
