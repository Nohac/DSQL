import { mkdtempSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { expect, test } from "bun:test";
import { DsqlDaemonError } from "../src/daemon";

test("formats daemon diagnostics with source line and column", () => {
  const dir = mkdtempSync(join(tmpdir(), "dsql-daemon-"));
  const file = join(dir, "query.tsx");
  writeFileSync(file, "const Movie = dsql`\nquery Movie {\n  movie_info {\n    id\n  }\n}\n`;\n");

  const error = new DsqlDaemonError(
    "cannot generate while diagnostics contain errors",
    [
      {
        file,
        range: { start: 20, end: 25 },
        embeddedRange: { start: 0, end: 5 },
        sourceOffset: 20,
        severity: "Error",
        source: "Check",
        code: "OutputKeyTooLong",
        message: "output key is too long",
      },
    ],
    [],
  );

  expect(error.diagnostics).toHaveLength(1);
  expect(error.message).toContain(
    `${file}:2:1 error Check OutputKeyTooLong: output key is too long (20..25)`,
  );
});

test("formats daemon diagnostics with byte range when source is unreadable", () => {
  const file = "/tmp/dsql-missing-query.tsx";
  const error = new DsqlDaemonError(
    "cannot generate while diagnostics contain errors",
    [
      {
        file,
        range: { start: 30, end: 134 },
        embeddedRange: { start: 10, end: 114 },
        sourceOffset: 20,
        severity: "Error",
        source: "Check",
        code: "OutputKeyTooLong",
        message: "output key is too long",
      },
    ],
    [],
  );

  expect(error.message).toContain(
    `${file}:30..134 error Check OutputKeyTooLong: output key is too long`,
  );
});
