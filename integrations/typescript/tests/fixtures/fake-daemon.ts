// A scripted stand-in for `dsql daemon`: consumes one step from the
// script file per request and answers with its canned response. Used by
// the client and Vite-binding tests; the real daemon is exercised by the
// DSQL_BIN-gated integration test.
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";

type Step = {
  readonly expectMethod?: string;
  readonly response?: { result?: unknown; error?: unknown };
  /** Emit a non-JSON line instead of answering. */
  readonly garbage?: boolean;
  /** Answer with a mismatched id. */
  readonly wrongId?: boolean;
  /** Die without answering. */
  readonly exit?: number;
};

// argv, not env: parallel test files must not share configuration
// through the process-global environment.
const scriptPath = process.argv[2];
if (!scriptPath) {
  process.exit(3);
}
const logPath = process.argv[3];
if (logPath) {
  writeFileSync(`${logPath}.args`, JSON.stringify(process.argv.slice(4)));
}
const steps = JSON.parse(readFileSync(scriptPath, "utf8")) as Step[];
let cursor = 0;

const lines = createInterface({ input: process.stdin });
lines.on("line", (line) => {
  if (line.trim() === "") {
    return;
  }
  const request = JSON.parse(line) as { id: number; method: string; params?: unknown };
  if (logPath) {
    appendFileSync(logPath, `${line}\n`);
  }
  if (request.method === "shutdown" && cursor >= steps.length) {
    process.stdout.write(`${JSON.stringify({ id: request.id, result: true })}\n`);
    process.exit(0);
  }
  const step = steps[cursor];
  cursor += 1;
  if (!step) {
    process.stdout.write(
      `${JSON.stringify({
        id: request.id,
        error: { code: "Internal", message: "fake daemon script exhausted", data: null },
      })}\n`,
    );
    return;
  }
  if (step.exit !== undefined) {
    process.exit(step.exit);
  }
  if (step.garbage) {
    process.stdout.write("this is not json\n");
    return;
  }
  if (step.expectMethod && step.expectMethod !== request.method) {
    process.stdout.write(
      `${JSON.stringify({
        id: request.id,
        error: {
          code: "Internal",
          message: `fake daemon expected ${step.expectMethod}, got ${request.method}`,
          data: null,
        },
      })}\n`,
    );
    return;
  }
  const id = step.wrongId ? request.id + 1000 : request.id;
  process.stdout.write(`${JSON.stringify({ id, ...(step.response ?? { result: null }) })}\n`);
  if (request.method === "shutdown") {
    process.exit(0);
  }
});
lines.on("close", () => {
  process.exit(0);
});
