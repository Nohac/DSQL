import { expect, test } from "bun:test";
import type { DsqlCallsite } from "../src/daemon";
import type { DsqlRenderModule } from "../src/node";
import {
  DsqlRewriteError,
  importInsertionByteOffset,
  rewriteCallsites,
} from "../src/rewrite";

function byteIndexOf(code: string, needle: string): number {
  const index = code.indexOf(needle);
  if (index < 0) {
    throw new Error(`fixture is missing ${needle}`);
  }
  return Buffer.byteLength(code.slice(0, index), "utf8");
}

function callsiteFor(
  code: string,
  expressions: Array<{ needle: string; id: string }>,
  path = "src/components/Panel.tsx",
): DsqlCallsite {
  return {
    path,
    contentHash: { algorithm: "sha256", value: "unused-by-rewrite" },
    expressions: expressions
      .map(({ needle, id }) => {
        const start = byteIndexOf(code, needle);
        return {
          range: { start, end: start + Buffer.byteLength(needle, "utf8") },
          definitions: [
            { kind: "query" as const, name: id.split("/")[2] ?? "Q", id },
          ],
        };
      })
      .sort((left, right) => left.range.start - right.range.start),
  };
}

const MODULES: ReadonlyMap<string, DsqlRenderModule> = new Map([
  [
    "frontend/operation/TitlePanel",
    {
      id: "frontend/operation/TitlePanel",
      module: "src/generated/dsql/queries/TitlePanel.ts",
      export: "TitlePanelOperation",
    },
  ],
  [
    "frontend/operation/HeroPanel",
    {
      id: "frontend/operation/HeroPanel",
      module: "src/generated/dsql/queries/HeroPanel.ts",
      export: "HeroPanelOperation",
    },
  ],
]);

const specifierFor = (module: DsqlRenderModule) => `/${module.module}`;

test("import insertion lands after shebang and directive prologue", () => {
  const code = '#!/usr/bin/env bun\n"use client";\n// før — multibyte comment\nconst x = 1;\n';
  const offset = importInsertionByteOffset(code, "module.ts");
  const bytes = Buffer.from(code, "utf8");
  expect(bytes.subarray(offset).toString("utf8")).toStartWith("const x = 1;");
});

test("directive prologue with multibyte content converts to byte offsets", () => {
  const code = '"useæ客户端";\nconst x = 1;\n';
  const offset = importInsertionByteOffset(code, "module.ts");
  const bytes = Buffer.from(code, "utf8");
  expect(bytes.subarray(offset).toString("utf8")).toStartWith("const x = 1;");
});

test("empty and directive-only modules insert at the end", () => {
  expect(importInsertionByteOffset("", "module.ts")).toBe(0);
  const directives = '"use client";\n';
  expect(importInsertionByteOffset(directives, "module.ts")).toBe(
    Buffer.byteLength(directives, "utf8"),
  );
});

test("rewrites expressions in byte space with multibyte text before them", () => {
  const expression = "dsql(`query TitlePanel { title { id } }`)";
  const code = `"use client";\n// tittel på norsk: blåbærsyltetøy 🫐\nexport const TitlePanelQuery = ${expression};\n`;
  const callsite = callsiteFor(code, [
    { needle: expression, id: "frontend/operation/TitlePanel" },
  ]);

  const rewritten = rewriteCallsites({
    code,
    path: "src/components/Panel.tsx",
    callsite,
    modules: MODULES,
    specifierFor,
  });

  expect(rewritten).toContain(
    'import { TitlePanelOperation as __dsql_TitlePanelOperation } from "/src/generated/dsql/queries/TitlePanel.ts";',
  );
  expect(rewritten).toContain(
    "export const TitlePanelQuery = __dsql_TitlePanelOperation;",
  );
  expect(rewritten).not.toContain("dsql(`");
  // The import block follows the directive and the file comment (the
  // parser skips leading trivia), never byte zero.
  expect(
    rewritten.startsWith(
      '"use client";\n// tittel på norsk: blåbærsyltetøy 🫐\nimport { TitlePanelOperation',
    ),
  ).toBe(true);
});

test("multiple expressions splice descending and share hoisted imports", () => {
  const first = "dsql(`query TitlePanel { title { id } }`)";
  const second = "dsql(`query HeroPanel { hero { id } }`)";
  const third = "dsql(`query TitlePanel { title { id } }`) ";
  const code = `const a = ${first};\nconst b = ${second};\nconst c = ${third.trim()};\n`;
  // a and c share the TitlePanel mapping.
  const callsite: DsqlCallsite = {
    path: "src/x.ts",
    contentHash: { algorithm: "sha256", value: "unused" },
    expressions: [
      {
        range: {
          start: byteIndexOf(code, first),
          end: byteIndexOf(code, first) + Buffer.byteLength(first),
        },
        definitions: [
          { kind: "query", name: "TitlePanel", id: "frontend/operation/TitlePanel" },
        ],
      },
      {
        range: {
          start: byteIndexOf(code, second),
          end: byteIndexOf(code, second) + Buffer.byteLength(second),
        },
        definitions: [
          { kind: "query", name: "HeroPanel", id: "frontend/operation/HeroPanel" },
          { kind: "fragment", name: "HeroBits", id: "frontend/fragment/HeroBits" },
        ],
      },
      {
        range: {
          start: code.lastIndexOf(first),
          end: code.lastIndexOf(first) + Buffer.byteLength(first),
        },
        definitions: [
          { kind: "query", name: "TitlePanel", id: "frontend/operation/TitlePanel" },
        ],
      },
    ],
  };

  const rewritten = rewriteCallsites({
    code,
    path: "src/x.ts",
    callsite,
    modules: MODULES,
    specifierFor,
  });
  expect(rewritten).toContain("const a = __dsql_TitlePanelOperation;");
  expect(rewritten).toContain("const b = __dsql_HeroPanelOperation;");
  expect(rewritten).toContain("const c = __dsql_TitlePanelOperation;");
  // One import per (module, export) pair, fragments need none.
  expect(rewritten.match(/import \{ TitlePanelOperation/g)).toHaveLength(1);
  expect(rewritten.match(/import \{ HeroPanelOperation/g)).toHaveLength(1);
});

test("local names avoid textual collisions", () => {
  const expression = "dsql(`query TitlePanel { title { id } }`)";
  const code = `const __dsql_TitlePanelOperation = 1;\nconst q = ${expression};\n`;
  const callsite = callsiteFor(code, [
    { needle: expression, id: "frontend/operation/TitlePanel" },
  ]);
  const rewritten = rewriteCallsites({
    code,
    path: "src/x.ts",
    callsite,
    modules: MODULES,
    specifierFor,
  });
  expect(rewritten).toContain("const q = __dsql_TitlePanelOperation_1;");
  expect(rewritten).toContain("TitlePanelOperation as __dsql_TitlePanelOperation_1");
});

test("a missing render mapping fails deterministically with the artifact id", () => {
  const expression = "dsql(`query Unknown { x { id } }`)";
  const code = `const q = ${expression};\n`;
  const callsite = callsiteFor(code, [
    { needle: expression, id: "frontend/operation/Unknown" },
  ]);
  expect(() =>
    rewriteCallsites({
      code,
      path: "src/x.ts",
      callsite,
      modules: MODULES,
      specifierFor,
    }),
  ).toThrow(DsqlRewriteError);
  expect(() =>
    rewriteCallsites({
      code,
      path: "src/x.ts",
      callsite,
      modules: MODULES,
      specifierFor,
    }),
  ).toThrow("no render mapping for artifact frontend/operation/Unknown");
});

test("out-of-bounds ranges are rejected instead of silently clamped", () => {
  const code = "const q = 1;\n";
  const callsite: DsqlCallsite = {
    path: "src/x.ts",
    contentHash: { algorithm: "sha256", value: "unused" },
    expressions: [
      {
        range: { start: 5, end: 5000 },
        definitions: [
          { kind: "query", name: "TitlePanel", id: "frontend/operation/TitlePanel" },
        ],
      },
    ],
  };
  expect(() =>
    rewriteCallsites({
      code,
      path: "src/x.ts",
      callsite,
      modules: MODULES,
      specifierFor,
    }),
  ).toThrow("exceeds the module's");
});
