import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
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

type DaemonResponse = {
  readonly id: number;
  readonly result?: unknown;
  readonly error?: {
    readonly message?: string;
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
      request.reject(new Error(response.error.message ?? "dsql daemon error"));
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
