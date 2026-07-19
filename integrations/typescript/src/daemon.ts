import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import type { BuildManifest, FragmentMetadata, OperationMetadata } from "./generated/metadata.ts";

/** The protocol version this client speaks (docs/spec/build-daemon.md). */
export const DSQL_PROTOCOL_VERSION = 1;

const STDERR_LIMIT = 64 * 1024;
const BACKOFF_INITIAL_MS = 250;
const BACKOFF_MAX_MS = 8_000;
const SHUTDOWN_TIMEOUT_MS = 3_000;

export type DsqlRange = {
  readonly start: number;
  readonly end: number;
};

export type DsqlContentHash = {
  readonly algorithm: "sha256";
  readonly value: string;
};

export type DsqlCallsiteExpression = {
  /** The whole `dsql(...)` expression in host UTF-8 byte offsets. */
  readonly range: DsqlRange;
  /** Stable opaque artifact id selected by the compiler. */
  readonly target: string;
};

export type DsqlCallsite = {
  /** Project-base-relative host file path. */
  readonly path: string;
  /** The binding responsible for rewriting this host. */
  readonly resolver: string;
  /** SHA-256 over the exact bytes the extractor read. */
  readonly contentHash: DsqlContentHash;
  readonly expressions: readonly DsqlCallsiteExpression[];
};

export type DsqlDiagnostic = {
  readonly file: string;
  readonly range: DsqlRange;
  readonly embeddedRange?: DsqlRange;
  readonly severity: "Error" | "Warning" | "Info";
  readonly source: string;
  readonly code: string;
  readonly message: string;
};

export type DsqlDiagnosticLevel = "error" | "warning" | "info";

export type DsqlArtifact = {
  readonly id: string;
  readonly kind: "operation" | "fragment";
  readonly scope: string;
  readonly metadata: OperationMetadata | FragmentMetadata;
};

export type DsqlArtifactGroup = {
  readonly name: string;
  readonly imports: readonly string[];
  /** The group's effective resolution closure, by artifact id. */
  readonly artifacts: readonly string[];
};

export type DsqlSourceFileScope = {
  readonly path: string;
  readonly scope: string;
};

export type DsqlCompileResult = {
  readonly generationId: number;
  readonly changed: boolean;
  readonly manifestPath: string;
  readonly currentManifestPath: string;
  readonly manifest: BuildManifest;
  readonly artifacts: readonly DsqlArtifact[];
  readonly groups: readonly DsqlArtifactGroup[];
  readonly sourceFileScopes: readonly DsqlSourceFileScope[];
  readonly callsites: readonly DsqlCallsite[];
  readonly diagnostics: readonly DsqlDiagnostic[];
};

export type DsqlInitializeResult = {
  readonly protocolVersion: number;
  readonly projectBase: string;
  readonly configPath: string;
  readonly schemaDir: string;
  readonly buildDir: string;
  readonly generatorOutputs: readonly string[];
  readonly diagnosticLevel: DsqlDiagnosticLevel;
};

export type DsqlDaemonOptions = {
  /** The daemon executable; default `dsql`. */
  readonly command?: string;
  /** Arguments; default `["daemon"]`. */
  readonly args?: readonly string[];
  readonly cwd?: string;
};

/** Adds one daemon CLI argument without changing explicit spawn settings. */
export function withDaemonArgument(
  options: DsqlDaemonOptions | undefined,
  argument: string,
  enabled: boolean,
): DsqlDaemonOptions | undefined {
  if (!enabled) {
    return options;
  }
  const args = [...(options?.args ?? ["daemon"])];
  if (!args.includes(argument)) {
    args.push(argument);
  }
  return { ...options, args };
}

export type DsqlDaemonClientOptions = {
  /** Absolute path the daemon discovers the project from. */
  readonly root: string;
  /** Renderer-owned roots, passed as `initialize` excludeRoots. */
  readonly excludeRoots?: readonly string[];
  /** Lowest diagnostic severity returned in compile snapshots. */
  readonly diagnosticLevel?: DsqlDiagnosticLevel;
  readonly daemon?: DsqlDaemonOptions;
  /**
   * Called when the session dies unexpectedly, BEFORE any restart:
   * the consumer must drop caches derived from earlier responses.
   */
  readonly onInvalidate?: () => void;
};

/**
 * A daemon error response. `data` is preserved structurally — rendering
 * (line:col formatting, overlays) belongs to the binding adapter, not
 * this transport layer.
 */
export class DsqlDaemonError extends Error {
  readonly code: string;
  readonly data: unknown;

  constructor(code: string, message: string, data: unknown) {
    super(`${code}: ${message}`);
    this.name = "DsqlDaemonError";
    this.code = code;
    this.data = data;
  }

  /** The diagnostics snapshot when `data` carries one. */
  get diagnostics(): readonly DsqlDiagnostic[] {
    const data = this.data as { diagnostics?: readonly DsqlDiagnostic[] } | null;
    return data?.diagnostics ?? [];
  }
}

/** The session (spawn) died or broke protocol; caches must be dropped. */
export class DsqlDaemonSessionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DsqlDaemonSessionError";
  }
}

type Pending = {
  readonly id: number;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
};

type Session = {
  readonly child: ChildProcessWithoutNullStreams;
  info: DsqlInitializeResult | null;
  pending: Pending | null;
  stderr: string;
  /** Set when `shutdown`/`dispose` initiated the exit. */
  closing: boolean;
  dead: boolean;
  /** Set once #failSession ran — failure handling is exactly-once. */
  failed: boolean;
};

/**
 * The daemon client: strict FIFO (one request in flight, callers are
 * serialized), lazy spawn + initialize, restart with bounded backoff and
 * cache invalidation on unexpected death. `UnsupportedProtocolVersion`
 * is fatal without restart — respawning cannot heal a version mismatch.
 */
export class DsqlDaemonClient {
  readonly #options: DsqlDaemonClientOptions;
  #session: Session | null = null;
  #fatal: Error | null = null;
  #closing = false;
  #chain: Promise<unknown> = Promise.resolve();
  #nextId = 1;
  #backoffMs = BACKOFF_INITIAL_MS;
  #restartNotBefore = 0;

  constructor(options: DsqlDaemonClientOptions) {
    this.#options = options;
  }

  /** The `initialize` result of the live session, if any. */
  get info(): DsqlInitializeResult | null {
    return this.#session?.info ?? null;
  }

  async compile(): Promise<DsqlCompileResult> {
    this.#assertOpen();
    return (await this.#enqueue(() => this.#request("compile", {}))) as DsqlCompileResult;
  }

  async filesChanged(paths: readonly string[]): Promise<DsqlCompileResult> {
    this.#assertOpen();
    return (await this.#enqueue(() =>
      this.#request("filesChanged", { paths }),
    )) as DsqlCompileResult;
  }

  /** Work arriving after `shutdown` is rejected, not queued behind it. */
  #assertOpen(): void {
    if (this.#closing) {
      throw new DsqlDaemonSessionError("dsql daemon client is shut down");
    }
  }

  /**
   * Graceful shutdown with timeout, then SIGKILL. Rides the FIFO: work
   * queued before the call drains first (against whatever session it
   * spawns), work arriving after it is rejected. Safe when no session
   * is live.
   */
  shutdown(timeoutMs: number = SHUTDOWN_TIMEOUT_MS): Promise<void> {
    this.#closing = true;
    return this.#enqueue(() => this.#shutdownNow(timeoutMs)) as Promise<void>;
  }

  /**
   * Restarts the daemon: graceful shutdown of the current session, then
   * respawn on next use. The binding calls this on project-config
   * changes so `initialize` reports fresh `generatorOutputs`.
   */
  async restart(): Promise<void> {
    await this.shutdown();
    this.#closing = false;
    this.#restartNotBefore = 0;
    this.#backoffMs = BACKOFF_INITIAL_MS;
  }

  async #shutdownNow(timeoutMs: number): Promise<void> {
    const session = this.#session;
    this.#session = null;
    if (!session || session.dead) {
      return;
    }
    session.closing = true;
    try {
      await Promise.race([
        this.#send(session, "shutdown", {}),
        delay(timeoutMs).then(() => {
          throw new Error("shutdown timed out");
        }),
      ]);
    } catch {
      // Fall through to the kill below.
    }
    if (!session.dead) {
      const exited = new Promise<void>((resolve) => {
        session.child.once("exit", () => resolve());
      });
      session.child.kill();
      await Promise.race([exited, delay(timeoutMs)]);
      if (!session.dead) {
        session.child.kill("SIGKILL");
      }
    }
  }

  #enqueue(task: () => Promise<unknown>): Promise<unknown> {
    const run = this.#chain.then(task, task);
    // The chain swallows outcomes; callers get them from `run`.
    this.#chain = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  async #request(method: string, params: unknown): Promise<unknown> {
    const session = await this.#ensureSession();
    try {
      const result = await this.#send(session, method, params);
      // A full round-trip is the health signal that resets the backoff.
      this.#backoffMs = BACKOFF_INITIAL_MS;
      return result;
    } catch (error) {
      if (error instanceof DsqlDaemonError && error.code !== "Internal") {
        // An error RESPONSE (Diagnostics, ProjectLoadFailed, …) is still
        // a responsive daemon; only session failures keep the ladder.
        this.#backoffMs = BACKOFF_INITIAL_MS;
      }
      throw error;
    }
  }

  async #ensureSession(): Promise<Session> {
    if (this.#fatal) {
      throw this.#fatal;
    }
    if (this.#session && !this.#session.dead) {
      return this.#session;
    }
    this.#session = null;

    const wait = this.#restartNotBefore - Date.now();
    if (wait > 0) {
      await delay(wait);
    }

    const session = this.#spawn();
    this.#session = session;
    try {
      const info = (await this.#send(session, "initialize", {
        protocolVersion: DSQL_PROTOCOL_VERSION,
        root: this.#options.root,
        diagnosticLevel: this.#options.diagnosticLevel ?? "info",
        ...(this.#options.excludeRoots?.length
          ? { excludeRoots: this.#options.excludeRoots }
          : {}),
      })) as DsqlInitializeResult;
      session.info = info;
      return session;
    } catch (error) {
      if (error instanceof DsqlDaemonError && error.code === "UnsupportedProtocolVersion") {
        this.#fatal = error;
      }
      // An alive-but-uninitialized daemon is unusable; drop it — and a
      // persistent initialize failure must back off like a crash, not
      // retry hot. Transport failures already scheduled through
      // #failSession; advancing twice would skip ladder rungs.
      if (!session.failed) {
        this.#scheduleBackoff();
      }
      session.closing = true;
      session.child.kill();
      if (this.#session === session) {
        this.#session = null;
      }
      throw error;
    }
  }

  /**
   * The next spawn deadline is set when a session FAILS — a spawn that
   * initializes but crashes on every compile must still climb the
   * exponential ladder (250ms doubling to 8s). Only a healthy
   * compile/filesChanged result resets it.
   */
  #scheduleBackoff(): void {
    this.#restartNotBefore = Date.now() + this.#backoffMs;
    this.#backoffMs = Math.min(this.#backoffMs * 2, BACKOFF_MAX_MS);
  }

  #spawn(): Session {
    const daemon = this.#options.daemon ?? {};
    const child = spawn(daemon.command ?? "dsql", daemon.args ?? ["daemon"], {
      stdio: ["pipe", "pipe", "pipe"],
      cwd: daemon.cwd ?? this.#options.root,
    });
    const session: Session = {
      child,
      info: null,
      pending: null,
      stderr: "",
      closing: false,
      dead: false,
      failed: false,
    };

    child.stderr.on("data", (chunk: Buffer) => {
      session.stderr = (session.stderr + chunk.toString("utf8")).slice(-STDERR_LIMIT);
    });
    child.on("error", (error) => {
      this.#failSession(session, `failed to start dsql daemon: ${error.message}`);
    });
    child.on("exit", (code, signal) => {
      session.dead = true;
      if (session.closing) {
        return;
      }
      const detail = session.stderr.trim();
      const reason = signal ?? `exit code ${code}`;
      this.#failSession(
        session,
        detail
          ? `dsql daemon stopped with ${reason}: ${detail}`
          : `dsql daemon stopped with ${reason}`,
      );
    });
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      this.#receive(session, line);
    });
    lines.on("close", () => {
      // stdout can end without a process exit (fd closed, wedged
      // process): a session that can no longer answer is dead.
      if (!session.dead && !session.closing) {
        this.#failSession(session, "dsql daemon closed its stdout");
      }
    });
    return session;
  }

  #receive(session: Session, line: string): void {
    if (session.dead || line.trim() === "") {
      return;
    }
    let response: { id?: unknown; result?: unknown; error?: unknown };
    try {
      response = JSON.parse(line) as typeof response;
    } catch {
      this.#failSession(session, `dsql daemon sent malformed JSON: ${truncate(line)}`);
      return;
    }
    const pending = session.pending;
    if (!pending || response.id !== pending.id) {
      // Strict FIFO with one in-flight request: any other id is a
      // protocol breach, not a message to ignore.
      this.#failSession(
        session,
        `dsql daemon answered id ${String(response.id)} out of order`,
      );
      return;
    }
    session.pending = null;
    if (response.error && typeof response.error === "object") {
      const wire = response.error as { code?: string; message?: string; data?: unknown };
      const error = new DsqlDaemonError(
        wire.code ?? "Unknown",
        wire.message ?? "dsql daemon error",
        wire.data ?? null,
      );
      if (error.code === "Internal") {
        // The daemon's own invariants broke; its resident state is not
        // trustworthy. Fail the session (restartable), then report.
        pending.reject(error);
        this.#failSession(session, `dsql daemon reported an internal error: ${error.message}`);
        return;
      }
      pending.reject(error);
      return;
    }
    pending.resolve(response.result);
  }

  #send(session: Session, method: string, params: unknown): Promise<unknown> {
    if (session.dead) {
      return Promise.reject(new DsqlDaemonSessionError("dsql daemon session is closed"));
    }
    if (session.pending) {
      return Promise.reject(
        new DsqlDaemonSessionError("dsql daemon client sent overlapping requests"),
      );
    }
    if (this.#nextId >= Number.MAX_SAFE_INTEGER) {
      return Promise.reject(new DsqlDaemonSessionError("dsql request ids exhausted"));
    }
    const id = this.#nextId++;
    const payload = `${JSON.stringify({ id, method, params })}\n`;
    return new Promise((resolve, reject) => {
      session.pending = { id, resolve, reject };
      session.child.stdin.write(payload, (error) => {
        if (error && session.pending?.id === id) {
          session.pending = null;
          reject(error);
        }
      });
    });
  }

  /**
   * Kills the session, rejects in-flight work, and invalidates caches.
   * Exactly-once per session: an explicit failure kills the child, and
   * the resulting `exit` event must not invalidate again (a second late
   * invalidation could wipe state a replacement session produced).
   */
  #failSession(session: Session, message: string): void {
    if (session.failed) {
      return;
    }
    session.failed = true;
    this.#scheduleBackoff();
    const wasDead = session.dead;
    session.dead = true;
    const pending = session.pending;
    session.pending = null;
    pending?.reject(new DsqlDaemonSessionError(message));
    if (this.#session === session) {
      this.#session = null;
    }
    if (!wasDead) {
      session.child.kill();
    }
    if (!session.closing) {
      this.#options.onInvalidate?.();
    }
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function truncate(line: string): string {
  return line.length > 200 ? `${line.slice(0, 200)}…` : line;
}
