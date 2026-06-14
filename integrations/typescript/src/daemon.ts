import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import type { GeneratedArtifacts } from "./node.ts";

export type DsqlDaemonOptions = {
  readonly command?: string;
  readonly args?: readonly string[];
};

export type DsqlDaemon = {
  compileProject(root: string): Promise<GeneratedArtifacts>;
  close(): Promise<void>;
};

type PendingRequest = {
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
};

export type DsqlDiagnosticRange = {
  readonly start: number;
  readonly end: number;
};

export type DsqlDaemonDiagnostic = {
  readonly file: string;
  readonly range: DsqlDiagnosticRange;
  readonly embeddedRange?: DsqlDiagnosticRange;
  readonly sourceOffset: number;
  readonly severity: "Error" | "Warning" | "Info";
  readonly source: "Parse" | "Lower" | "Check" | "Lint" | "Plan" | "Format" | "Generate";
  readonly code: string;
  readonly message: string;
};

export type DsqlGenerationError = {
  readonly kind: string;
  readonly message: string;
  readonly file?: string;
  readonly range?: DsqlDiagnosticRange;
  readonly embeddedRange?: DsqlDiagnosticRange;
  readonly sourceOffset?: number;
  readonly source: "Generate";
  readonly code: string;
};

export class DsqlDaemonError extends Error {
  readonly diagnostics: readonly DsqlDaemonDiagnostic[];
  readonly generationErrors: readonly DsqlGenerationError[];

  constructor(
    message: string,
    diagnostics: readonly DsqlDaemonDiagnostic[] = [],
    generationErrors: readonly DsqlGenerationError[] = [],
  ) {
    super(formatDsqlDaemonErrorMessage(message, diagnostics, generationErrors));
    this.name = "DsqlDaemonError";
    this.diagnostics = diagnostics;
    this.generationErrors = generationErrors;
  }
}

type DaemonResponse = {
  readonly id: number;
  readonly result?: unknown;
  readonly error?: {
    readonly message?: string;
    readonly diagnostics?: readonly DsqlDaemonDiagnostic[];
    readonly errors?: readonly DsqlGenerationError[];
  };
};

export function startDsqlDaemon(options: DsqlDaemonOptions = {}): DsqlDaemon {
  const child = spawn(options.command ?? "dsql", options.args ?? ["daemon"], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  const pending = new Map<number, PendingRequest>();
  let nextId = 1;
  let stderr = "";
  let closed = false;

  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString("utf8");
  });

  child.on("error", (error) => {
    closed = true;
    rejectAll(
      pending,
      new Error(`failed to start dsql daemon: ${error.message}`),
    );
  });

  child.on("exit", (code, signal) => {
    closed = true;
    const detail = stderr.trim();
    const reason = signal ?? `exit code ${code}`;
    const message = detail
      ? `dsql daemon stopped with ${reason}: ${detail}`
      : `dsql daemon stopped with ${reason}`;
    for (const request of pending.values()) {
      request.reject(new Error(message));
    }
    pending.clear();
  });

  createInterface({ input: child.stdout }).on("line", (line) => {
    let response: DaemonResponse;
    try {
      response = JSON.parse(line) as DaemonResponse;
    } catch (error) {
      rejectAll(
        pending,
        new Error(`failed to parse dsql daemon response: ${String(error)}`),
      );
      return;
    }

    const request = pending.get(response.id);
    if (!request) {
      return;
    }
    pending.delete(response.id);
    if (response.error) {
      const diagnostics = response.error.diagnostics ?? [];
      const generationErrors = response.error.errors ?? [];
      request.reject(
        diagnostics.length > 0 || generationErrors.length > 0
          ? new DsqlDaemonError(
              response.error.message ?? "dsql daemon error",
              diagnostics,
              generationErrors,
            )
          : new Error(response.error.message ?? "dsql daemon error"),
      );
    } else {
      request.resolve(response.result);
    }
  });

  const request = (method: string, params: unknown): Promise<unknown> => {
    if (closed || child.stdin.writableEnded || child.stdin.destroyed) {
      return Promise.reject(new Error("dsql daemon is closed"));
    }

    const id = nextId++;
    const payload = `${JSON.stringify({ id, method, params })}\n`;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      try {
        child.stdin.write(payload, (error) => {
          if (error) {
            pending.delete(id);
            reject(error);
          }
        });
      } catch (error) {
        pending.delete(id);
        reject(error);
      }
    });
  };

  return {
    async compileProject(root: string): Promise<GeneratedArtifacts> {
      return (await request("compileProject", { root })) as GeneratedArtifacts;
    },
    async close(): Promise<void> {
      if (closed) {
        return;
      }
      closed = true;
      child.stdin.end();
      if (child.exitCode !== null) {
        return;
      }
      child.kill();
    },
  };
}

function rejectAll(pending: Map<number, PendingRequest>, error: Error): void {
  for (const request of pending.values()) {
    request.reject(error);
  }
  pending.clear();
}

function formatDsqlDaemonErrorMessage(
  message: string,
  diagnostics: readonly DsqlDaemonDiagnostic[],
  generationErrors: readonly DsqlGenerationError[],
): string {
  const lines = [message];
  for (const diagnostic of diagnostics) {
    lines.push(formatDiagnosticLine(diagnostic));
  }
  for (const error of generationErrors) {
    lines.push(formatGenerationErrorLine(error));
  }
  return lines.join("\n");
}

function formatDiagnosticLine(diagnostic: DsqlDaemonDiagnostic): string {
  return `${formatLocation(diagnostic.file, diagnostic.range)} ${diagnostic.severity.toLowerCase()} ${diagnostic.source} ${diagnostic.code}: ${diagnostic.message} (${diagnostic.range.start}..${diagnostic.range.end})`;
}

function formatGenerationErrorLine(error: DsqlGenerationError): string {
  const location =
    error.file && error.range
      ? formatLocation(error.file, error.range)
      : "project";
  const range = error.range ? ` (${error.range.start}..${error.range.end})` : "";
  return `${location} error ${error.source} ${error.code}: ${error.message}${range}`;
}

function formatLocation(file: string, range: DsqlDiagnosticRange): string {
  const position = byteOffsetToPosition(file, range.start);
  if (!position) {
    return `${file}:${range.start}..${range.end}`;
  }
  return `${file}:${position.line}:${position.column}`;
}

function byteOffsetToPosition(
  file: string,
  offset: number,
): { readonly line: number; readonly column: number } | undefined {
  let text: string;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    return undefined;
  }

  let byte = 0;
  let line = 1;
  let column = 1;
  for (const char of text) {
    const charBytes = Buffer.byteLength(char, "utf8");
    if (byte + charBytes > offset) {
      break;
    }
    byte += charBytes;
    if (char === "\n") {
      line += 1;
      column = 1;
    } else {
      column += 1;
    }
  }
  return { line, column };
}
