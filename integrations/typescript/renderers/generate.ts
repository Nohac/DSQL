import {
  defineDsqlGenerator,
  renderDsqlHelper,
  renderTypes,
  runDsqlGeneratorFromEnv,
} from "@dsql/typescript/node";
import { renderTanStackQuery } from "./generators/tanstack-query";
import { renderTanStackStart } from "./generators/tanstack-start";

const generator = defineDsqlGenerator(async ({ artifacts, outDir }) => {
  await renderTypes(artifacts, { outDir });
  await renderDsqlHelper(artifacts, { outDir });
  await renderTanStackStart(artifacts, { outDir });
  await renderTanStackQuery(artifacts, { outDir });
});

export default generator;

await runDsqlGeneratorFromEnv(generator);
