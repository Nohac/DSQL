import {
  defineDsqlGenerator,
  renderDsql,
  runDsqlGeneratorFromEnv,
} from "@dsql/typescript/node";
import { renderTanStackQuery } from "./generators/tanstack-query";
import { renderTanStackStart } from "./generators/tanstack-start";

const generator = defineDsqlGenerator(async ({ artifacts, root, outDir }) => {
  const dsql = await renderDsql(artifacts, {
    root,
    queriesDir: outDir,
  });
  await renderTanStackStart(artifacts, dsql, { root, outDir });
  await renderTanStackQuery(artifacts, dsql, { root, outDir });
  return dsql;
});

export default generator;

await runDsqlGeneratorFromEnv(generator);
