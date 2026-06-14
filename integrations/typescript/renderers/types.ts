import {
  loadBuildArtifacts,
  renderDsql,
} from "@dsql/typescript/node";
import { dirname, join } from "node:path";

const manifestPath = process.env.DSQL_MANIFEST;

if (!manifestPath) {
  throw new Error("DSQL_MANIFEST is required");
}

const artifacts = loadBuildArtifacts(manifestPath);
const root = dirname(dirname(dirname(manifestPath)));

await renderDsql(artifacts, {
  root,
  queriesDir: join(root, "src/generated/dsql"),
});
