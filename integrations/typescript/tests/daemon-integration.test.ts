// End-to-end against the real daemon binary. Gated: set DSQL_BIN (the
// `test:daemon` script builds the workspace binary and sets it).
import { cpSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "bun:test";
import { DsqlDaemonClient } from "../src/daemon";
import {
  buildArtifactsFromGenerated,
  embeddedDefinitionsOf,
  resolveEmbeddedSources,
} from "../src/node";

const BIN = process.env.DSQL_BIN ? resolve(process.env.DSQL_BIN) : null;
const FIXTURE = join(import.meta.dir, "../../../crates/dsql-project/tests/it/fixture/scoped");
const integration = BIN ? test : test.skip;

integration("the real daemon serves compile results this package understands", async () => {
  const projectDir = mkdtempSync(join(tmpdir(), "dsql-integration-"));
  cpSync(FIXTURE, projectDir, { recursive: true });

  const client = new DsqlDaemonClient({
    root: projectDir,
    daemon: { command: BIN as string, args: ["daemon"], cwd: projectDir },
  });
  try {
    const result = await client.compile();
    expect(result.generationId).toBe(1);
    expect(result.changed).toBe(true);
    expect(client.info?.protocolVersion).toBe(1);

    const projectBase = client.info?.projectBase ?? projectDir;
    const artifacts = buildArtifactsFromGenerated(result, { projectBase });
    expect(artifacts.operations.length).toBeGreaterThan(0);
    expect(artifacts.artifactIds.size).toBe(result.artifacts.length);

    // The embedded host round-trips: callsite hash matches disk, the
    // extractor's content_range slices real dsql text.
    const embedded = resolveEmbeddedSources(embeddedDefinitionsOf(artifacts), {
      projectBase,
      callsites: result.callsites,
    });
    expect(embedded.mismatches).toEqual([]);
    const titlePanel = embedded.sources.get("operation/TitlePanel");
    expect(titlePanel).toContain("query TitlePanel");

    const callsite = result.callsites.find((entry) =>
      entry.path.endsWith("TitlePanel.ts"),
    );
    expect(callsite).toBeDefined();
    expect(callsite?.resolver).toBe("typescript");
    expect(callsite?.expressions[0]?.target).toBe("frontend/operation/TitlePanel");

    // A no-op batch replays the outcome without a new generation.
    const replay = await client.filesChanged(["README.md"]);
    expect(replay.changed).toBe(false);
    expect(replay.generationId).toBe(result.generationId);
  } finally {
    await client.shutdown();
  }
});
