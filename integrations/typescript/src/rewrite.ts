import ts from "typescript";
import type { DsqlCallsite } from "./daemon.ts";
import type { DsqlRenderModule } from "./node.ts";

/**
 * Callsite rewriting (docs/spec/build-daemon.md, Callsites and
 * freshness): expression ranges are UTF-8 byte offsets into the raw
 * file, so every splice happens in byte space — descending, one decode
 * at the end. The import block is inserted after the shebang and the
 * directive prologue, located with the TypeScript parser (never a
 * regex), and the parser's offset unit is probed once rather than
 * assumed.
 */

export class DsqlRewriteError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DsqlRewriteError";
  }
}

/** `string index in code` → `byte offset`, installed by the probe. */
type OffsetConverter = (code: string, index: number) => number;

let offsetConverter: OffsetConverter | null = null;

/**
 * Determines whether the parser reports positions in UTF-16 code units
 * or UTF-8 bytes by parsing a fixed multibyte snippet. Fails
 * deterministically if the result matches neither — silently choosing
 * one would corrupt every splice after a multibyte character.
 */
function probeOffsetConverter(): OffsetConverter {
  if (offsetConverter) {
    return offsetConverter;
  }
  const snippet = '"å";const x = 1;';
  const parsed = ts.createSourceFile(
    "probe.ts",
    snippet,
    ts.ScriptTarget.Latest,
    false,
    ts.ScriptKind.TS,
  );
  const second = parsed.statements[1];
  const start = second?.getStart(parsed);
  if (start === 4) {
    // UTF-16 code-unit indices ("å" is one unit, two bytes).
    offsetConverter = (code, index) => Buffer.byteLength(code.slice(0, index), "utf8");
  } else if (start === 5) {
    offsetConverter = (_code, index) => index;
  } else {
    throw new DsqlRewriteError(
      `the TypeScript parser reported offset ${String(start)} for a probe ` +
        "statement expected at 4 (UTF-16) or 5 (UTF-8); refusing to guess " +
        "the offset unit",
    );
  }
  return offsetConverter;
}

/**
 * The byte offset where hoisted imports belong: after the shebang and
 * the directive prologue (`"use client"` and friends), before the first
 * real statement.
 */
export function importInsertionByteOffset(code: string, path: string): number {
  const convert = probeOffsetConverter();
  const parsed = ts.createSourceFile(
    path,
    code,
    ts.ScriptTarget.Latest,
    false,
  );
  for (const statement of parsed.statements) {
    if (
      ts.isExpressionStatement(statement) &&
      ts.isStringLiteral(statement.expression)
    ) {
      continue;
    }
    return convert(code, statement.getStart(parsed));
  }
  return Buffer.byteLength(code, "utf8");
}

export type RewriteOptions = {
  /** The raw module source — the exact bytes `contentHash` covers. */
  readonly code: string;
  /** Vite-cleaned module path, for messages and TypeScript parsing. */
  readonly path: string;
  readonly callsite: DsqlCallsite;
  /** Render mappings by artifact id. */
  readonly modules: ReadonlyMap<string, DsqlRenderModule>;
  /** Derives the host import specifier for a mapped module. */
  readonly specifierFor: (module: DsqlRenderModule) => string;
};

/**
 * Replaces every callsite expression with a hoisted-import reference.
 * The daemon supplies one opaque artifact target per expression.
 */
export function rewriteCallsites(options: RewriteOptions): string {
  const { code, path, callsite } = options;
  if (callsite.resolver !== "typescript") {
    throw new DsqlRewriteError(
      `${path}: dsql resolver ${JSON.stringify(callsite.resolver)} is not ` +
        "supported by @dsql/typescript; assign the host to resolver " +
        '"typescript" or install the matching binding',
    );
  }
  let bytes = Buffer.from(code, "utf8");

  // Collision-free local bindings: any textual occurrence disqualifies
  // a candidate, which over-rejects but never captures.
  const localNames = new Map<string, string>();
  const localFor = (module: DsqlRenderModule): string => {
    const key = `${module.module}#${module.export}`;
    const existing = localNames.get(key);
    if (existing) {
      return existing;
    }
    let candidate = `__dsql_${module.export}`;
    let counter = 0;
    while (code.includes(candidate) || [...localNames.values()].includes(candidate)) {
      counter += 1;
      candidate = `__dsql_${module.export}_${counter}`;
    }
    localNames.set(key, candidate);
    return candidate;
  };

  const splices: Array<{ start: number; end: number; text: string }> = [];
  const used = new Map<string, DsqlRenderModule>();
  for (const expression of callsite.expressions) {
    const module = options.modules.get(expression.target);
    if (!module) {
      throw new DsqlRewriteError(
        `${path}: no render mapping for artifact ${expression.target} — the renderer ` +
          "did not produce a module for it",
      );
    }
    used.set(`${module.module}#${module.export}`, module);
    splices.push({
      start: expression.range.start,
      end: expression.range.end,
      text: localFor(module),
    });
  }
  if (splices.length === 0) {
    return code;
  }

  splices.sort((left, right) => right.start - left.start);
  for (const splice of splices) {
    if (splice.end > bytes.length || splice.start > splice.end) {
      throw new DsqlRewriteError(
        `${path}: callsite range ${splice.start}..${splice.end} exceeds the ` +
          `module's ${bytes.length} bytes`,
      );
    }
    bytes = Buffer.concat([
      bytes.subarray(0, splice.start),
      Buffer.from(splice.text, "utf8"),
      bytes.subarray(splice.end),
    ]);
  }

  const imports = [...used.values()]
    .map((module) => {
      const local = localFor(module);
      const clause =
        local === module.export ? module.export : `${module.export} as ${local}`;
      return `import { ${clause} } from ${JSON.stringify(options.specifierFor(module))};\n`;
    })
    .sort()
    .join("");
  // Expressions always live at or after the prologue boundary, so the
  // insertion offset (computed on the original code) is unaffected by
  // the descending splices above.
  const insertAt = importInsertionByteOffset(code, path);
  bytes = Buffer.concat([
    bytes.subarray(0, insertAt),
    Buffer.from(imports, "utf8"),
    bytes.subarray(insertAt),
  ]);
  return bytes.toString("utf8");
}
