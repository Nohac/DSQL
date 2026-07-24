import { runDsqlRendererFromProject } from "@dsql/typescript/node";
import {
  targetOutput,
  typescriptDefinitions,
} from "@dsql/typescript/renderer";
import { tanstackQuery } from "./generators/tanstack-query";
import { tanstackStart } from "./generators/tanstack-start";
import { project } from "./project.generated";

const generators = () => [
  project.generator(typescriptDefinitions({ executionDir: "queries.server" })),
  project.generator(tanstackStart()),
  project.generator(tanstackQuery()),
];

export const renderer = project.renderer({
  output: targetOutput("src/generated/dsql"),
  targets: {
    api: { generators: generators() },
    frontend: { generators: generators() },
  },
});

export default renderer;

if (import.meta.main) {
  await runDsqlRendererFromProject(renderer, {
    ...(process.env.DSQL_BIN ? { daemon: { command: process.env.DSQL_BIN } } : {}),
  });
}
