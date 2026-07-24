import {
  loadBuildArtifacts,
  projectRelative,
  reconcileDsqlOutputs,
  renderDsql,
} from "@dsql/typescript/node";
import { dirname, join } from "node:path";

// The minimal flat-manifest generator (legacy `[generate.typescript]`
// cmd channel): environment-only, no groups, no registry augmentation.
const manifestPath = process.env.DSQL_MANIFEST;

if (!manifestPath) {
  throw new Error("DSQL_MANIFEST is required");
}

const artifacts = loadBuildArtifacts(manifestPath);
const root = process.env.DSQL_PROJECT_DIR ?? dirname(dirname(dirname(manifestPath)));

const rendered = await renderDsql(artifacts, {
  root,
  queriesDir: join(root, "src/generated/dsql"),
});
reconcileDsqlOutputs({
  projectBase: root,
  ownedRoots: ["src/generated/dsql"],
  files: rendered.files.map((file) => ({
    path: projectRelative(root, file.path),
    contents: file.contents,
  })),
});
