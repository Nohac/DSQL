import {
  defineDsqlRenderer,
  renderDsql,
  renderMapFromResults,
  runDsqlRendererFromProject,
  type DsqlRenderResult,
} from "@dsql/typescript/node";
import path from "node:path";
import { renderTanStackQuery } from "./generators/tanstack-query";
import { renderTanStackStart } from "./generators/tanstack-start";

type ScopeRenderPaths = {
  readonly queriesDir: string;
  readonly executionDir?: string;
};

const renderPathsByScope: Record<string, ScopeRenderPaths | undefined> = {
  default: {
    queriesDir: "queries",
    executionDir: "queries.server",
  },
  frontend: {
    queriesDir: "queries",
    executionDir: "queries.server",
  },
};

const OWNED_ROOTS = ["src/generated/dsql"] as const;

export const renderer = defineDsqlRenderer({
  ownedRoots: OWNED_ROOTS,
  async render({ projectBase, artifacts, embeddedSources, mode, command }) {
    void mode;
    void command;
    const outDir = path.join(projectBase, "src/generated/dsql");
    const dsqlResults: DsqlRenderResult[] = [];
    const extraFiles: string[] = [];
    const groups =
      artifacts.artifactGroups.length > 0 ? artifacts.artifactGroups : [artifacts];

    // Two groups writing the same layout would fight over one ownership
    // manifest — the second render removes the first render's files.
    const layoutOwners = new Map<string, string>();

    for (const contextArtifacts of groups) {
      const scope = contextArtifacts.scopes[0] ?? { name: "default", imports: [] };
      const paths = renderPathsByScope[scope.name];
      if (!paths) {
        continue;
      }
      const owner = layoutOwners.get(paths.queriesDir);
      if (owner !== undefined) {
        throw new Error(
          `scopes ${owner} and ${scope.name} both render into ` +
            `${paths.queriesDir}; give each scope its own output directory`,
        );
      }
      layoutOwners.set(paths.queriesDir, scope.name);

      const renderOptions = {
        root: projectBase,
        scope,
        queriesDir: path.join(outDir, paths.queriesDir),
        ...(paths.executionDir
          ? { executionDir: path.join(outDir, paths.executionDir) }
          : {}),
        embeddedSources,
      };
      const dsql = await renderDsql(contextArtifacts, renderOptions);

      extraFiles.push(
        ...(await renderTanStackStart(contextArtifacts, dsql, {
          root: projectBase,
          outDir,
        })),
        ...(await renderTanStackQuery(contextArtifacts, dsql, {
          root: projectBase,
          outDir,
        })),
      );

      dsqlResults.push(dsql);
    }

    return renderMapFromResults(dsqlResults, {
      projectBase,
      ownedRoots: OWNED_ROOTS,
      extraFiles,
    });
  },
});

export default renderer;

if (import.meta.main) {
  await runDsqlRendererFromProject(renderer);
}
