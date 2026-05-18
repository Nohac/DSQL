import { readFileSync } from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import type {
  BuildManifest,
  OperationManifestEntry,
  OperationMetadata,
} from "./generated/metadata";

export { renderDsqlHelper, renderTypes } from "./render/types";
export type { RenderOptions } from "./render/types";

export type BuildArtifacts = {
  readonly manifestPath: string;
  readonly manifest: BuildManifest;
  readonly operations: OperationMetadata[];
  readonly operationsByName: ReadonlyMap<string, OperationMetadata>;
};

export function loadBuildArtifacts(manifestPath: string): BuildArtifacts {
  const manifest = readBuildManifest(manifestPath);
  const operations = manifest.operations.map((entry) =>
    readOperationMetadata(manifestPath, entry),
  );
  return {
    manifestPath,
    manifest,
    operations,
    operationsByName: new Map(
      operations.map((operation) => [operation.name, operation]),
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

export function resolveArtifactPath(baseDir: string, path: string): string {
  return isAbsolute(path) ? path : join(baseDir, path);
}

function readJson(path: string): unknown {
  return JSON.parse(readFileSync(path, "utf8"));
}
