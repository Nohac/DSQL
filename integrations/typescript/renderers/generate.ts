import {
  defineDsqlGenerator,
  renderDsql,
  runDsqlGeneratorFromEnv,
} from "@dsql/typescript/node";
import path from "node:path";
import { renderTanStackQuery } from "./generators/tanstack-query";
import { renderTanStackStart } from "./generators/tanstack-start";

const generator = defineDsqlGenerator(async ({ artifacts, root }) => {
  const outDir = path.join(root, "src/generated/dsql");
  const dsql = await renderDsql(artifacts, {
    root,
    queriesDir: path.join(outDir, "queries"),
    executionDir: path.join(outDir, "queries.server"),
  });
  await renderTanStackStart(artifacts, dsql, {
    root,
    outDir,
  });
  await renderTanStackQuery(artifacts, dsql, {
    root,
    outDir,
  });
  return dsql;
});

export default generator;

await runDsqlGeneratorFromEnv(generator);
