import {
  loadBuildArtifacts,
  renderDsql,
} from "@dsql/typescript/node";
import { dirname } from "node:path";

const manifestPath = process.env.DSQL_MANIFEST;
const outDir = process.env.DSQL_OUT_DIR;

if (!manifestPath || !outDir) {
  throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
}

const artifacts = loadBuildArtifacts(manifestPath);

await renderDsql(artifacts, {
  root: dirname(dirname(dirname(manifestPath))),
  queriesDir: outDir,
});
