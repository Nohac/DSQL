import {
  defineDsqlGenerator,
  type DsqlRenderResult,
  renderDsql,
  runDsqlGeneratorFromEnv,
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

const generator = defineDsqlGenerator(async ({ artifacts, root }) => {
  const outDir = path.join(root, "src/generated/dsql");
  const dsqlResults: DsqlRenderResult[] = [];
  const groups =
    artifacts.artifactGroups.length > 0 ? artifacts.artifactGroups : [artifacts];

  for (const contextArtifacts of groups) {
    const scope = contextArtifacts.scopes[0] ?? { name: "default", imports: [] };
    const paths = renderPathsByScope[scope.name];
    if (!paths) {
      continue;
    }

    const renderOptions = {
      root,
      scope,
      queriesDir: path.join(outDir, paths.queriesDir),
      ...(paths.executionDir
        ? { executionDir: path.join(outDir, paths.executionDir) }
        : {}),
    };
    const dsql = await renderDsql(contextArtifacts, renderOptions);

    await renderTanStackStart(contextArtifacts, dsql, {
      root,
      outDir,
    });
    await renderTanStackQuery(contextArtifacts, dsql, {
      root,
      outDir,
    });

    dsqlResults.push(dsql);
  }

  return dsqlResults;
});

export default generator;

await runDsqlGeneratorFromEnv(generator);
