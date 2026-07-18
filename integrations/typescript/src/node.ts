import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import type {
  BuildManifest,
  FragmentManifestEntry,
  FragmentMetadata,
  OperationManifestEntry,
  OperationMetadata,
} from "./generated/metadata.ts";
import type {
  DsqlCallsite,
  DsqlCompileResult,
  DsqlDaemonOptions,
} from "./daemon.ts";
import { DsqlDaemonClient, withDaemonArgument } from "./daemon.ts";
import type { DsqlRenderResult } from "./render/types.ts";

export { DsqlDaemonClient, DsqlDaemonError, DsqlDaemonSessionError } from "./daemon.ts";
export type {
  DsqlArtifact,
  DsqlArtifactGroup,
  DsqlCallsite,
  DsqlCallsiteExpression,
  DsqlCompileResult,
  DsqlContentHash,
  DsqlDaemonClientOptions,
  DsqlDaemonOptions,
  DsqlDiagnostic,
  DsqlInitializeResult,
  DsqlRange,
  DsqlSourceFileScope,
} from "./daemon.ts";
export { artifactKey, renderDsql } from "./render/types.ts";
import { artifactKey } from "./render/types.ts";
export type {
  DsqlRenderDefinitionResult,
  DsqlRenderedFile,
  DsqlRenderResult,
  RenderDsqlOptions,
} from "./render/types.ts";

/**
 * Artifacts in the shape the render layer consumes: metadata lists plus
 * stable artifact ids keyed `family/name` (`operation/Titles`). Group
 * views carry the group's effective resolution closure.
 */
export type BuildArtifacts = {
  /** Absolute path of the immutable per-generation manifest. */
  readonly manifestPath: string;
  /** Absolute path of the fixed `manifest.json` pointer. */
  readonly currentManifestPath: string;
  readonly manifest: BuildManifest;
  readonly scopes: readonly GeneratedResolutionScope[];
  readonly sourceFileScopes: readonly GeneratedSourceScope[];
  readonly artifactGroups: readonly BuildArtifacts[];
  readonly operations: OperationMetadata[];
  readonly operationsByName: ReadonlyMap<string, OperationMetadata>;
  readonly fragments: FragmentMetadata[];
  readonly fragmentsByName: ReadonlyMap<string, FragmentMetadata>;
  /** `family/name` → full artifact id (`scope/family/name`). Empty for
   * flat-manifest loads, which carry no scope identity. */
  readonly artifactIds: ReadonlyMap<string, string>;
};

export type GeneratedResolutionScope = {
  readonly name: string;
  readonly imports: readonly string[];
};

export type GeneratedSourceScope = {
  readonly file: string;
  readonly scope: string;
};

/**
 * Adapts a daemon compile result into [`BuildArtifacts`]: validates that
 * every group reference resolves, builds per-group views (closure
 * artifacts appear once per view, manifest filtered to the closure), and
 * resolves the manifest paths against the initialized project base.
 */
export function buildArtifactsFromGenerated(
  result: DsqlCompileResult,
  options: { readonly projectBase: string },
): BuildArtifacts {
  const byId = new Map(result.artifacts.map((artifact) => [artifact.id, artifact]));
  for (const group of result.groups) {
    for (const id of group.artifacts) {
      if (!byId.has(id)) {
        throw new Error(
          `dsql compile result group ${group.name} references unknown artifact ${id}`,
        );
      }
    }
  }

  const manifestPath = resolve(options.projectBase, result.manifestPath);
  const currentManifestPath = resolve(options.projectBase, result.currentManifestPath);
  const view = (
    artifacts: readonly (typeof result.artifacts)[number][],
    scopes: readonly GeneratedResolutionScope[],
    manifest: BuildManifest,
    sourceFileScopes: readonly GeneratedSourceScope[],
    groups: readonly BuildArtifacts[],
  ): BuildArtifacts => {
    const operations = artifacts
      .filter((artifact) => artifact.kind === "operation")
      .map((artifact) => artifact.metadata as OperationMetadata);
    const fragments = artifacts
      .filter((artifact) => artifact.kind === "fragment")
      .map((artifact) => artifact.metadata as FragmentMetadata);
    return {
      manifestPath,
      currentManifestPath,
      manifest,
      scopes,
      sourceFileScopes,
      artifactGroups: groups,
      operations,
      operationsByName: new Map(operations.map((operation) => [operation.name, operation])),
      fragments,
      fragmentsByName: new Map(fragments.map((fragment) => [fragment.name, fragment])),
      artifactIds: new Map(
        artifacts.map((artifact) => [
          artifactKey(artifact.kind, artifact.metadata.name),
          artifact.id,
        ]),
      ),
    };
  };

  const groupViews = result.groups.map((group) => {
    const members = group.artifacts.map((id) => byId.get(id));
    const names = new Set(
      members.flatMap((artifact) => (artifact ? [artifact.metadata.name] : [])),
    );
    const manifest: BuildManifest = {
      ...result.manifest,
      operations: result.manifest.operations.filter((entry) => names.has(entry.name)),
      fragments: result.manifest.fragments.filter((entry) => names.has(entry.name)),
    };
    const memberPaths = new Set(
      result.sourceFileScopes
        .filter((entry) => entry.scope === group.name)
        .map((entry) => entry.path),
    );
    return view(
      members.flatMap((artifact) => (artifact ? [artifact] : [])),
      [{ name: group.name, imports: group.imports }],
      manifest,
      result.sourceFileScopes
        .filter((entry) => memberPaths.has(entry.path))
        .map((entry) => ({ file: entry.path, scope: entry.scope })),
      [],
    );
  });

  return view(
    [...result.artifacts],
    result.groups.map((group) => ({ name: group.name, imports: group.imports })),
    result.manifest,
    result.sourceFileScopes.map((entry) => ({ file: entry.path, scope: entry.scope })),
    groupViews,
  );
}

export type EmbeddedSourceResolution = {
  /** `family/name` → the exact template content, sliced by extractor
   * authority (`content_range`). */
  readonly sources: ReadonlyMap<string, string>;
  /** Host paths whose bytes no longer match the compile result's
   * `contentHash` — the caller decides whether to retry or fail. */
  readonly mismatches: readonly string[];
};

type EmbeddedDefinition = {
  readonly kind: "operation" | "fragment";
  readonly metadata: OperationMetadata | FragmentMetadata;
  readonly id?: string;
};

/**
 * Slices embedded definitions' template content from their host files
 * using the Rust-owned `content_range` — no detection, no scanning.
 *
 * With `callsites` (the daemon channel), every touched host is verified
 * against its `contentHash` first and reported in `mismatches` on drift.
 * Without them (the flat-manifest channel), only range bounds and UTF-8
 * code-point boundaries are validated — that path reads files moments
 * after a compile and has no hash to check against.
 */
export function resolveEmbeddedSources(
  definitions: readonly EmbeddedDefinition[],
  options: {
    readonly projectBase: string;
    readonly callsites?: readonly DsqlCallsite[];
  },
): EmbeddedSourceResolution {
  const files = new Map<string, Buffer | null>();
  const mismatches = new Set<string>();
  const sources = new Map<string, string>();
  const sourceEntry = (definition: (typeof definitions)[number]) =>
    definition.metadata.source_map.find(
      (candidate) => candidate.id === definition.metadata.name,
    );
  const slice = (
    definition: (typeof definitions)[number],
    expectedHash?: string,
  ): void => {
    const entry = definition.metadata.source_map.find(
      (candidate) => candidate.id === definition.metadata.name,
    );
    if (!entry?.content_range) {
      return;
    }
    let bytes = files.get(entry.file);
    if (bytes === undefined) {
      try {
        bytes = readFileSync(resolve(options.projectBase, entry.file));
      } catch {
        bytes = null;
      }
      files.set(entry.file, bytes);
    }
    if (bytes === null) {
      mismatches.add(entry.file);
      return;
    }
    if (expectedHash !== undefined && sha256Hex(bytes) !== expectedHash) {
      mismatches.add(entry.file);
      return;
    }
    const { start, end } = entry.content_range;
    if (start > end || end > bytes.length) {
      mismatches.add(entry.file);
      return;
    }
    const slice = bytes.subarray(start, end);
    const text = slice.toString("utf8");
    if (Buffer.byteLength(text, "utf8") !== slice.length) {
      // Not a code-point boundary: the file on disk diverged from the
      // compiled state.
      mismatches.add(entry.file);
      return;
    }
    sources.set(artifactKey(definition.kind, definition.metadata.name), text);
  };

  if (options.callsites !== undefined) {
    // Successful daemon results give every artifact an id and every target an artifact.
    const byId = new Map(
      definitions.map((definition) => [definition.id!, definition] as const),
    );
    for (const callsite of options.callsites) {
      for (const expression of callsite.expressions) {
        const definition = byId.get(expression.target)!;
        slice(definition, callsite.contentHash.value);
      }
    }
  } else {
    const groups = new Map<string, Array<(typeof definitions)[number]>>();
    for (const definition of definitions) {
      const entry = sourceEntry(definition);
      if (!entry?.content_range) {
        continue;
      }
      const key = `${entry.file}\0${entry.content_range.start}\0${entry.content_range.end}`;
      const group = groups.get(key) ?? [];
      group.push(definition);
      groups.set(key, group);
    }
    for (const group of groups.values()) {
      if (group.length === 1 && group[0]) {
        slice(group[0]);
      }
    }
  }
  return { sources, mismatches: [...mismatches].sort() };
}

/** [`resolveEmbeddedSources`] over a [`BuildArtifacts`]. */
export function embeddedDefinitionsOf(
  artifacts: BuildArtifacts,
): EmbeddedDefinition[] {
  const definition = (
    kind: EmbeddedDefinition["kind"],
    metadata: EmbeddedDefinition["metadata"],
  ): EmbeddedDefinition => {
    const id = artifacts.artifactIds.get(artifactKey(kind, metadata.name));
    return id ? { kind, metadata, id } : { kind, metadata };
  };
  return [
    ...artifacts.operations.map((metadata) => definition("operation", metadata)),
    ...artifacts.fragments.map((metadata) => definition("fragment", metadata)),
  ];
}

/**
 * The renderer descriptor a binding consumes (docs/spec/build-daemon.md,
 * Rewrite contract). `ownedRoots` is configuration known before any
 * invocation — the binding passes it as `initialize` excludeRoots and
 * excludes it from watching.
 */
export type DsqlRenderer = {
  /** Project-base-relative directories the renderer writes into. */
  readonly ownedRoots: readonly string[];
  render(context: DsqlRendererContext): Promise<DsqlRenderMap>;
};

export type DsqlRendererContext = {
  readonly projectBase: string;
  /** The full compile result, verbatim. */
  readonly result: DsqlCompileResult;
  /** The adapted view with per-group closures. */
  readonly artifacts: BuildArtifacts;
  /** `family/name` → exact template content for embedded definitions,
   * preflighted against the compile's content hashes. */
  readonly embeddedSources: ReadonlyMap<string, string>;
  readonly mode: string;
  readonly command: "serve" | "build";
};

export type DsqlRenderModule = {
  /** Stable artifact id (`scope/family/name`). */
  readonly id: string;
  /** Project-base-relative file path — the binding derives the
   * host-appropriate import specifier. */
  readonly module: string;
  /** Named export to reference. */
  readonly export: string;
};

export type DsqlRenderMap = {
  readonly modules: readonly DsqlRenderModule[];
  /** Repeats the descriptor's roots as a consistency check. */
  readonly ownedRoots: readonly string[];
  /** Every file this render wrote (including ownership manifests). */
  readonly files: readonly string[];
};

export function defineDsqlRenderer(renderer: DsqlRenderer): DsqlRenderer {
  return renderer;
}

/**
 * Validates a render map against its descriptor: owned-root set equality,
 * unique ids, identifier-shaped exports, and containment of every module
 * and file under an owned root.
 */
export function validateDsqlRenderMap(
  map: DsqlRenderMap,
  renderer: DsqlRenderer,
): void {
  const configured = new Set(
    map.ownedRoots.map((root) => assertProjectRelativePath(root, "owned root")),
  );
  const declared = new Set(
    renderer.ownedRoots.map((root) => assertProjectRelativePath(root, "owned root")),
  );
  if (
    configured.size !== declared.size ||
    [...configured].some((root) => !declared.has(root))
  ) {
    throw new Error(
      `dsql render map ownedRoots ${[...configured].join(", ")} differ from the ` +
        `renderer's declared roots ${[...declared].join(", ")}`,
    );
  }
  const ids = new Set<string>();
  for (const module of map.modules) {
    if (ids.has(module.id)) {
      throw new Error(`dsql render map maps artifact ${module.id} twice`);
    }
    ids.add(module.id);
    if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(module.export)) {
      throw new Error(
        `dsql render map export ${JSON.stringify(module.export)} for ${module.id} ` +
          "is not a valid identifier",
      );
    }
    if (!isUnderAny(assertProjectRelativePath(module.module, "module"), declared)) {
      throw new Error(
        `dsql render map module ${module.module} is outside every owned root`,
      );
    }
  }
  for (const file of map.files) {
    if (!isUnderAny(assertProjectRelativePath(file, "file"), declared)) {
      throw new Error(`dsql render map file ${file} is outside every owned root`);
    }
  }
}

/**
 * Composes a render map from [`renderDsql`] results plus any extra files
 * other generators wrote. Every rendered definition can be the opaque
 * target of one embedded expression.
 */
export function renderMapFromResults(
  results: readonly DsqlRenderResult[],
  options: {
    readonly projectBase: string;
    readonly ownedRoots: readonly string[];
    /** Absolute or project-base-relative paths other generators wrote. */
    readonly extraFiles?: readonly string[];
  },
): DsqlRenderMap {
  const relativize = (path: string) =>
    isAbsolute(path) ? projectRelative(options.projectBase, path) : path;
  const modules = new Map<string, DsqlRenderModule>();
  const files = new Set<string>((options.extraFiles ?? []).map(relativize));
  for (const result of results) {
    for (const file of result.files) {
      files.add(relativize(file.path));
    }
    for (const definition of Object.values(result.definitions)) {
      if (!definition.id) {
        throw new Error(
          `render result for ${definition.name} carries no artifact id; ` +
            "flat-manifest renders cannot feed a binding render map",
        );
      }
      const existing = modules.get(definition.id);
      const next: DsqlRenderModule = {
        id: definition.id,
        module: definition.modulePath,
        export: definition.exportName,
      };
      if (
        existing &&
        (existing.module !== next.module || existing.export !== next.export)
      ) {
        throw new Error(
          `render results map artifact ${definition.id} to both ` +
            `${existing.module}#${existing.export} and ${next.module}#${next.export}`,
        );
      }
      modules.set(definition.id, next);
    }
  }
  return {
    modules: [...modules.values()].sort((left, right) => left.id.localeCompare(right.id)),
    ownedRoots: options.ownedRoots,
    files: [...files].sort(),
  };
}

/**
 * Renders one compile result after hash-checking every embedded host. A
 * drifting host gets exactly one refresh; validation completes before the
 * render map becomes visible to the caller.
 */
export async function renderDsqlCompileResult(
  renderer: DsqlRenderer,
  initialResult: DsqlCompileResult,
  options: {
    readonly projectBase: string;
    readonly refresh: (paths: readonly string[]) => Promise<DsqlCompileResult>;
    readonly environment: () => Pick<DsqlRendererContext, "mode" | "command">;
  },
): Promise<{
  readonly result: DsqlCompileResult;
  readonly renderMap: DsqlRenderMap;
}> {
  const prepare = (result: DsqlCompileResult) => {
    const artifacts = buildArtifactsFromGenerated(result, {
      projectBase: options.projectBase,
    });
    const embedded = resolveEmbeddedSources(embeddedDefinitionsOf(artifacts), {
      projectBase: options.projectBase,
      callsites: result.callsites,
    });
    return { artifacts, embedded };
  };

  let result = initialResult;
  let prepared = prepare(result);
  if (prepared.embedded.mismatches.length > 0) {
    result = await options.refresh(prepared.embedded.mismatches);
    prepared = prepare(result);
    if (prepared.embedded.mismatches.length > 0) {
      throw new Error(
        `embedded hosts kept changing while rendering: ${prepared.embedded.mismatches.join(", ")}`,
      );
    }
  }

  const renderMap = await renderer.render({
    projectBase: options.projectBase,
    result,
    artifacts: prepared.artifacts,
    embeddedSources: prepared.embedded.sources,
    ...options.environment(),
  });
  validateDsqlRenderMap(renderMap, renderer);
  return { result, renderMap };
}

/**
 * Runs a renderer once against a freshly spawned daemon — the explicit
 * one-shot channel (`bun dsql/generate.ts`), giving it the same groups
 * and content hashes as a binding. Shuts the daemon down in `finally`.
 */
export async function runDsqlRendererFromProject(
  renderer: DsqlRenderer,
  options: {
    readonly root?: string;
    readonly daemon?: DsqlDaemonOptions;
    readonly mode?: string;
    /** Enforce the project's `dsql.lock` for this one-shot run. */
    readonly locked?: boolean;
  } = {},
): Promise<DsqlRenderMap> {
  const root = resolve(options.root ?? process.cwd());
  const daemon = withDaemonArgument(options.daemon, "--locked", options.locked ?? false);
  const client = new DsqlDaemonClient({
    root,
    excludeRoots: renderer.ownedRoots,
    ...(daemon ? { daemon } : {}),
  });
  try {
    const result = await client.compile();
    const projectBase = client.info?.projectBase ?? root;
    const rendered = await renderDsqlCompileResult(renderer, result, {
      projectBase,
      refresh: (paths) => client.filesChanged(paths),
      environment: () => ({
        mode: options.mode ?? process.env.NODE_ENV ?? "production",
        command: "build",
      }),
    });
    return rendered.renderMap;
  } finally {
    await client.shutdown();
  }
}

/**
 * The legacy flat-manifest generator contract, kept for
 * `[generate.typescript] cmd` compatibility: strictly environment-driven
 * (`DSQL_MANIFEST` + `DSQL_PROJECT_DIR`), no groups, no callsite hashes —
 * embedded-source slices are validated for bounds only.
 */
export type DsqlGeneratorContext = {
  readonly artifacts: BuildArtifacts;
  readonly root: string;
  readonly mode: string;
  readonly command: "serve" | "build";
};

export type DsqlGenerator = (
  context: DsqlGeneratorContext,
) => DsqlRenderResult | DsqlRenderResult[] | void | Promise<DsqlRenderResult | DsqlRenderResult[] | void>;

export function defineDsqlGenerator(generator: DsqlGenerator): DsqlGenerator {
  return generator;
}

export async function runDsqlGeneratorFromEnv(
  generator: DsqlGenerator,
): Promise<boolean> {
  const manifestPath = process.env.DSQL_MANIFEST;
  if (!manifestPath) {
    return false;
  }

  await generator({
    artifacts: loadBuildArtifacts(manifestPath),
    root: process.env.DSQL_PROJECT_DIR ?? dirname(dirname(dirname(manifestPath))),
    mode: process.env.NODE_ENV ?? "production",
    command: "build",
  });
  return true;
}

/** Loads the flat on-disk manifest (version 2) and its artifacts. */
export function loadBuildArtifacts(manifestPath: string): BuildArtifacts {
  const manifest = readBuildManifest(manifestPath);
  const operations = manifest.operations.map((entry) =>
    readOperationMetadata(manifestPath, entry),
  );
  const fragments = manifest.fragments.map((entry) =>
    readFragmentMetadata(manifestPath, entry),
  );
  return {
    manifestPath,
    // Flat loads are handed one manifest file; it doubles as the
    // pointer (one-shot callers pass `dsql/build/manifest.json`).
    currentManifestPath: manifestPath,
    manifest,
    scopes: [{ name: "default", imports: [] }],
    sourceFileScopes: [],
    artifactGroups: [],
    operations,
    operationsByName: new Map(operations.map((operation) => [operation.name, operation])),
    fragments,
    fragmentsByName: new Map(fragments.map((fragment) => [fragment.name, fragment])),
    artifactIds: new Map(),
  };
}

export function readBuildManifest(path: string): BuildManifest {
  return readJson(path) as BuildManifest;
}

export function readOperationMetadata(
  manifestPath: string,
  entry: OperationManifestEntry,
): OperationMetadata {
  return readJson(resolveArtifactPath(dirname(manifestPath), entry.path)) as OperationMetadata;
}

export function readFragmentMetadata(
  manifestPath: string,
  entry: FragmentManifestEntry,
): FragmentMetadata {
  return readJson(resolveArtifactPath(dirname(manifestPath), entry.path)) as FragmentMetadata;
}

export function resolveArtifactPath(baseDir: string, path: string): string {
  return isAbsolute(path) ? path : join(baseDir, path);
}

function readJson(path: string): unknown {
  return JSON.parse(readFileSync(path, "utf8"));
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function normalizeRelative(path: string): string {
  return path.split("\\").join("/").replace(/\/+$/, "").replace(/^\.\//, "");
}

/**
 * Render-map paths must be plain project-base-relative paths: no
 * absolutes, no drive letters, no `.`/`..` segments — containment
 * checks on traversable paths would be bypassable.
 */
function assertProjectRelativePath(path: string, role: string): string {
  const normalized = normalizeRelative(path);
  const segments = normalized.split("/");
  if (
    normalized === "" ||
    isAbsolute(path) ||
    /^[A-Za-z]:/.test(path) ||
    segments.some((segment) => segment === "" || segment === "." || segment === "..")
  ) {
    throw new Error(
      `dsql render map ${role} ${JSON.stringify(path)} is not a plain ` +
        "project-base-relative path",
    );
  }
  return normalized;
}

function isUnderAny(path: string, roots: ReadonlySet<string>): boolean {
  const normalized = normalizeRelative(path);
  for (const root of roots) {
    if (normalized === root || normalized.startsWith(`${root}/`)) {
      return true;
    }
  }
  return false;
}

/** Relates an absolute path back to the project base with `/` separators. */
export function projectRelative(projectBase: string, path: string): string {
  return normalizeRelative(relative(projectBase, path));
}
