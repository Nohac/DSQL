import { readFileSync } from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import type { DsqlRenderResult } from "./render/types.js";
import type {
  BuildManifest,
  FragmentManifestEntry,
  FragmentMetadata,
  OperationManifestEntry,
  OperationMetadata,
} from "./generated/metadata";
import { renderDsql, renderDsqlHelper, renderTypes } from "./render/types.js";

export { renderDsql, renderDsqlHelper, renderTypes } from "./render/types.js";
export type {
  DsqlRenderDefinitionResult,
  DsqlRenderedFile,
  DsqlRenderResult,
  RenderDsqlOptions,
  RenderOptions,
} from "./render/types.js";

export type BuildArtifacts = {
  readonly manifestPath: string;
  readonly manifest: BuildManifest;
  readonly scopes: readonly GeneratedResolutionScope[];
  readonly sourceFileScopes: readonly GeneratedSourceScope[];
  readonly artifactGroups: readonly BuildArtifacts[];
  readonly operations: OperationMetadata[];
  readonly operationsByName: ReadonlyMap<string, OperationMetadata>;
  readonly fragments: FragmentMetadata[];
  readonly fragmentsByName: ReadonlyMap<string, FragmentMetadata>;
};

export type GeneratedOperationArtifact = {
  readonly metadata: OperationMetadata;
  readonly hash: string;
  readonly source: string;
};

export type GeneratedFragmentArtifact = {
  readonly metadata: FragmentMetadata;
  readonly hash: string;
  readonly source: string;
};

export type GeneratedResolutionScope = {
  readonly name: string;
  readonly imports: string[];
};

export type GeneratedSourceScope = {
  readonly file: string;
  readonly source_offset: number;
  readonly scope: string;
};

export type GeneratedArtifacts = {
  readonly project_dir: string;
  readonly out_dir: string;
  readonly manifest_path: string;
  readonly scopes?: GeneratedResolutionScope[];
  readonly source_file_scopes?: GeneratedSourceScope[];
  readonly manifest: BuildManifest;
  readonly operations: GeneratedOperationArtifact[];
  readonly fragments: GeneratedFragmentArtifact[];
  readonly artifact_groups?: GeneratedArtifactGroup[];
};

export type GeneratedArtifactGroup = {
  readonly name: string;
  readonly imports: string[];
  readonly manifest: BuildManifest;
  readonly operations: GeneratedOperationArtifact[];
  readonly fragments: GeneratedFragmentArtifact[];
  readonly source_file_scopes: GeneratedSourceScope[];
};

export type DsqlGeneratorContext = {
  readonly artifacts: BuildArtifacts;
  readonly root: string;
  readonly outDir: string;
  readonly mode: string;
  readonly command: "serve" | "build";
};

export type DsqlGeneratorResult = DsqlRenderResult | DsqlRenderResult[] | void;

export type DsqlGenerator = (
  context: DsqlGeneratorContext,
) => DsqlGeneratorResult | Promise<DsqlGeneratorResult>;

export function defineDsqlGenerator(generator: DsqlGenerator): DsqlGenerator {
  return generator;
}

export async function runDsqlGeneratorFromEnv(
  generator: DsqlGenerator,
): Promise<boolean> {
  const manifestPath = process.env.DSQL_MANIFEST;
  const outDir = process.env.DSQL_OUT_DIR;
  if (!manifestPath && !outDir) {
    return false;
  }
  if (!manifestPath || !outDir) {
    throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
  }

  await generator({
    artifacts: loadBuildArtifacts(manifestPath),
    root: dirname(dirname(dirname(manifestPath))),
    outDir,
    mode: process.env.NODE_ENV ?? "production",
    command: "build",
  });
  return true;
}

export const defaultDsqlGenerator = defineDsqlGenerator(
  async ({ artifacts, root, outDir }) => {
    if (artifacts.artifactGroups.length > 0) {
      return Promise.all(
        artifacts.artifactGroups.map((group) =>
          renderDsql(group, {
            root,
            queriesDir: join(outDir, group.scopes[0]?.name ?? "default", "queries"),
            executionDir: join(
              outDir,
              group.scopes[0]?.name ?? "default",
              "queries.server",
            ),
          }),
        ),
      );
    }
    return renderDsql(artifacts, {
      root,
      queriesDir: outDir,
    });
  },
);

export function buildArtifactsFromGenerated(
  generated: GeneratedArtifacts,
): BuildArtifacts {
  const operations = generated.operations.map((operation) => operation.metadata);
  const fragments = generated.fragments.map((fragment) => fragment.metadata);
  const scopes = generated.scopes ?? [{ name: "default", imports: [] }];
  const sourceFileScopes = generated.source_file_scopes ?? [];
  const manifestPath = generated.manifest_path;
  const artifactGroups =
    generated.artifact_groups?.map((group) =>
      buildArtifactsFromGroup(generated, group),
    ) ?? [];
  return {
    manifestPath,
    manifest: generated.manifest,
    scopes,
    sourceFileScopes,
    artifactGroups,
    operations,
    operationsByName: new Map(
      operations.map((operation) => [operation.name, operation]),
    ),
    fragments,
    fragmentsByName: new Map(
      fragments.map((fragment) => [fragment.name, fragment]),
    ),
  };
}

function buildArtifactsFromGroup(
  generated: GeneratedArtifacts,
  group: GeneratedArtifactGroup,
): BuildArtifacts {
  const operations = group.operations.map((operation) => operation.metadata);
  const fragments = group.fragments.map((fragment) => fragment.metadata);
  const scope = { name: group.name, imports: group.imports };
  return {
    manifestPath: generated.manifest_path,
    manifest: group.manifest,
    scopes: [scope],
    sourceFileScopes: group.source_file_scopes,
    artifactGroups: [],
    operations,
    operationsByName: new Map(
      operations.map((operation) => [operation.name, operation]),
    ),
    fragments,
    fragmentsByName: new Map(
      fragments.map((fragment) => [fragment.name, fragment]),
    ),
  };
}

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
    manifest,
    scopes: [{ name: "default", imports: [] }],
    sourceFileScopes: [],
    artifactGroups: [],
    operations,
    operationsByName: new Map(
      operations.map((operation) => [operation.name, operation]),
    ),
    fragments,
    fragmentsByName: new Map(
      fragments.map((fragment) => [fragment.name, fragment]),
    ),
  };
}

export function readBuildManifest(path: string): BuildManifest {
  return readJson(path) as BuildManifest;
}

export function readOperationMetadata(
  manifestPath: string,
  entry: OperationManifestEntry,
): OperationMetadata {
  return readJson(
    resolveArtifactPath(dirname(manifestPath), entry.path),
  ) as OperationMetadata;
}

export function readFragmentMetadata(
  manifestPath: string,
  entry: FragmentManifestEntry,
): FragmentMetadata {
  return readJson(
    resolveArtifactPath(dirname(manifestPath), entry.path),
  ) as FragmentMetadata;
}

export function resolveArtifactPath(baseDir: string, path: string): string {
  return isAbsolute(path) ? path : join(baseDir, path);
}

function readJson(path: string): unknown {
  return JSON.parse(readFileSync(path, "utf8"));
}
