import {
  loadBuildArtifacts,
  renderDsqlHelper,
  renderTypes,
} from "@dsql/typescript/node";
import { renderTanStackQuery } from "./generators/tanstack-query";
import { renderTanStackStart } from "./generators/tanstack-start";

const manifestPath = process.env.DSQL_MANIFEST;
const outDir = process.env.DSQL_OUT_DIR;

if (!manifestPath || !outDir) {
  throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
}

const artifacts = loadBuildArtifacts(manifestPath);

await renderTypes(artifacts, { outDir });
await renderDsqlHelper(artifacts, { outDir });
await renderTanStackStart(artifacts, { outDir });
await renderTanStackQuery(artifacts, { outDir });
