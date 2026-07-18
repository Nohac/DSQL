import { resolve } from "node:path";
import type { Plugin, ResolvedConfig, ViteDevServer } from "vite";
import {
  DsqlDaemonClient,
  DsqlDaemonError,
  type DsqlCallsite,
  type DsqlCompileResult,
  type DsqlDaemonOptions,
  type DsqlDiagnostic,
} from "./daemon.ts";
import {
  projectRelative,
  renderDsqlCompileResult,
  sha256Hex,
  type DsqlRenderer,
  type DsqlRenderMap,
  type DsqlRenderModule,
} from "./node.ts";
import { DsqlRewriteError, rewriteCallsites } from "./rewrite.ts";

export type DsqlVitePluginOptions = {
  /** The renderer descriptor; its `ownedRoots` become watch exclusions
   * and `initialize` excludeRoots. */
  readonly renderer: DsqlRenderer;
  readonly daemon?: DsqlDaemonOptions;
  /** Project discovery root; defaults to the Vite root. */
  readonly root?: string;
  /** Send a full reload after changed recompiles (default true). */
  readonly fullReload?: boolean;
};

type CompileOutcome =
  | { readonly kind: "success"; readonly result: DsqlCompileResult }
  | { readonly kind: "unchanged"; readonly result: DsqlCompileResult }
  | { readonly kind: "error"; readonly error: Error };

/** The atomically swapped consequence of a successful compile+render. */
type ReadyState = {
  readonly result: DsqlCompileResult;
  readonly renderMap: DsqlRenderMap;
  readonly modulesById: ReadonlyMap<string, DsqlRenderModule>;
  readonly callsitesByPath: ReadonlyMap<string, DsqlCallsite>;
};

export function dsql(options: DsqlVitePluginOptions): Plugin {
  const fullReload = options.fullReload ?? true;
  let config: ResolvedConfig | null = null;
  // Captured in the config hook — the initial compile can render before
  // the config is resolved, and the renderer must see the real command.
  let env: { command: "build" | "serve"; mode: string } | null = null;
  let server: ViteDevServer | null = null;
  // The pre-resolution Vite root from the config hook (9a): the initial
  // compile can run before configResolved.
  let configRoot: string | null = null;
  let client: DsqlDaemonClient | null = null;
  let state: ReadyState | null = null;
  // Invalidation epoch: bumped whenever cached results stop being
  // trustworthy (daemon death, config reload). A render that started
  // under an older epoch must not swap its state in.
  let epoch = 0;
  let initial: Promise<CompileOutcome> | null = null;
  // The last surfaced compile error, replayed once a logger/server
  // exists (initial compiles can fail before either does).
  let lastError: Error | null = null;

  // Coalescing loop: one outstanding compile; changes arriving meanwhile
  // batch into the next request. Waiters resolve with the outcome of the
  // batch that carried their path.
  const pendingPaths = new Set<string>();
  let pendingWaiters: Array<(outcome: CompileOutcome) => void> = [];
  let activeBatch: {
    readonly paths: ReadonlySet<string>;
    readonly promise: Promise<CompileOutcome>;
  } | null = null;
  let loop: Promise<void> | null = null;

  const projectRoot = () =>
    resolve(options.root ?? config?.root ?? configRoot ?? process.cwd());

  const invalidate = (): void => {
    // Cached results may describe a world a restarted daemon or a
    // reloaded config will re-derive differently. Stop transforming
    // until the next successful compile; in-flight renders from older
    // epochs are discarded at the swap.
    epoch += 1;
    state = null;
    initial = null;
  };

  const ensureClient = (): DsqlDaemonClient => {
    if (!client) {
      client = new DsqlDaemonClient({
        root: projectRoot(),
        excludeRoots: options.renderer.ownedRoots,
        ...(options.daemon ? { daemon: options.daemon } : {}),
        onInvalidate: invalidate,
      });
    }
    return client;
  };

  const exclusions = (): string[] => {
    const info = ensureClient().info;
    return [
      ...options.renderer.ownedRoots,
      info?.buildDir ?? "dsql/build",
      ...(info?.generatorOutputs ?? []),
    ].map((root) => root.replace(/\/+$/, ""));
  };

  const isExcluded = (path: string): boolean => {
    if (path.startsWith("node_modules/") || path.includes("/node_modules/")) {
      return true;
    }
    if (path.startsWith(".git/") || path.includes("/.git/")) {
      return true;
    }
    return exclusions().some((root) => path === root || path.startsWith(`${root}/`));
  };

  const projectBase = () => ensureClient().info?.projectBase ?? projectRoot();

  const surfaceError = (error: Error): void => {
    lastError = error;
    const message =
      error instanceof DsqlDaemonError
        ? renderDaemonError(error, projectBase())
        : error.message;
    config?.logger.error(`[dsql] ${message}`);
    server?.ws.send({
      type: "error",
      err: { message: `[dsql] ${message}`, stack: error.stack ?? "" },
    });
  };

  /** Runs one compile for `paths` (empty = the initial full compile). */
  const compileBatch = async (paths: readonly string[]): Promise<CompileOutcome> => {
    const daemon = ensureClient();
    try {
      const info = daemon.info;
      const configChanged =
        info !== null && paths.some((path) => path === info.configPath);
      if (configChanged) {
        // generatorOutputs can change with the config, and only
        // `initialize` reports them: invalidate BEFORE restarting so a
        // failed reload cannot leave old-scope transforms active, then
        // compile once with the refreshed exclusion set.
        invalidate();
        await daemon.restart();
      }
      const startEpoch = epoch;
      const result =
        paths.length === 0 || configChanged || daemon.info === null
          ? await daemon.compile()
          : await daemon.filesChanged(paths);

      if (result.changed === false && state !== null) {
        return { kind: "unchanged", result };
      }
      const next = await renderAndIndex(result);
      if (epoch !== startEpoch) {
        // The session was invalidated while rendering: this result may
        // describe a daemon that no longer exists. Discard it.
        throw new Error("dsql session invalidated during render; recompile pending");
      }
      state = next;
      lastError = null;
      // The render preflight may have refreshed the result; report the
      // one actually swapped in.
      return { kind: "success", result: next.result };
    } catch (error) {
      const failure = error instanceof Error ? error : new Error(String(error));
      surfaceError(failure);
      return { kind: "error", error: failure };
    }
  };

  /**
   * Renders and indexes a successful compile. The embedded-source
   * preflight verifies every touched host against its content hash,
   * retries drifted hosts through one `filesChanged` round-trip, and
   * fails the whole render on persistent drift — nothing is written and
   * the previous state stays (atomic swap only on success).
   */
  const renderAndIndex = async (first: DsqlCompileResult): Promise<ReadyState> => {
    const daemon = ensureClient();
    const base = projectBase();
    const { result, renderMap } = await renderDsqlCompileResult(
      options.renderer,
      first,
      {
        projectBase: base,
        refresh: (paths) => daemon.filesChanged(paths),
        environment: () => ({
          mode: env?.mode ?? config?.mode ?? "development",
          command:
            (env?.command ?? config?.command) === "build" ? "build" : "serve",
        }),
      },
    );
    return {
      result,
      renderMap,
      modulesById: new Map(renderMap.modules.map((module) => [module.id, module])),
      callsitesByPath: new Map(
        result.callsites.map((callsite) => [callsite.path, callsite]),
      ),
    };
  };

  const runLoop = (): void => {
    if (loop) {
      return;
    }
    loop = (async () => {
      while (pendingPaths.size > 0) {
        // One microtask of settling: a single fs event fans out to both
        // the watcher listener and handleHotUpdate in the same tick —
        // draining synchronously would hide the queued path from the
        // second listener and double-deliver the change.
        await Promise.resolve();
        const batch = [...pendingPaths];
        pendingPaths.clear();
        const waiters = pendingWaiters;
        pendingWaiters = [];
        let announce: (outcome: CompileOutcome) => void = () => undefined;
        const promise = new Promise<CompileOutcome>((resolveOutcome) => {
          announce = resolveOutcome;
        });
        activeBatch = { paths: new Set(batch), promise };
        const outcome = await compileBatch(batch);
        announce(outcome);
        activeBatch = null;
        for (const waiter of waiters) {
          waiter(outcome);
        }
        if (outcome.kind === "success" && fullReload && server) {
          server.ws.send({ type: "full-reload" });
        }
      }
      loop = null;
    })();
  };

  /** Queues paths and resolves with the outcome of their batch. */
  const enqueue = (paths: readonly string[]): Promise<CompileOutcome> => {
    for (const path of paths) {
      pendingPaths.add(path);
    }
    const promise = new Promise<CompileOutcome>((resolveOutcome) => {
      pendingWaiters.push(resolveOutcome);
    });
    runLoop();
    return promise;
  };

  /**
   * Joins the batch already carrying `path` (queued or in flight) or
   * enqueues it — never delivers the same change twice.
   */
  const outcomeFor = (path: string): Promise<CompileOutcome> => {
    if (pendingPaths.has(path)) {
      return new Promise<CompileOutcome>((resolveOutcome) => {
        pendingWaiters.push(resolveOutcome);
      });
    }
    if (activeBatch?.paths.has(path)) {
      return activeBatch.promise;
    }
    return enqueue([path]);
  };

  const ensureInitial = (): Promise<CompileOutcome> => {
    initial ??= compileBatch([]);
    return initial;
  };

  const normalizeId = (id: string): string | null => {
    const clean = id.split("?")[0]?.split("#")[0] ?? id;
    if (!clean.startsWith("/") && !/^[A-Za-z]:[/\\]/.test(clean)) {
      return null;
    }
    const relativePath = projectRelative(projectBase(), clean);
    if (relativePath.startsWith("..")) {
      return null;
    }
    return relativePath;
  };

  const specifierFor = (module: DsqlRenderModule): string => {
    const absolute = resolve(projectBase(), module.module);
    const viteRoot = config?.root ?? projectRoot();
    const rootRelative = projectRelative(viteRoot, absolute);
    return rootRelative.startsWith("..")
      ? `/@fs/${absolute.split("\\").join("/")}`
      : `/${rootRelative}`;
  };

  return {
    name: "dsql",
    enforce: "pre",

    async config(userConfig, configEnv) {
      env = {
        command: configEnv.command === "build" ? "build" : "serve",
        mode: configEnv.mode,
      };
      if (userConfig.root) {
        configRoot = resolve(userConfig.root);
      }
      // Initialize early so buildDir + generatorOutputs join the watch
      // exclusions from the start; compile failures surface on
      // buildStart (compileBatch never throws).
      await ensureInitial();
      return {
        server: {
          watch: {
            ignored: exclusions().map(
              (root) => `${resolve(projectBase(), root).split("\\").join("/")}/**`,
            ),
          },
        },
      };
    },

    configResolved(resolved) {
      config = resolved;
    },

    configureServer(devServer) {
      server = devServer;
      // Initial compile errors can predate the logger and the dev
      // server: replay the latest one so it is not silently lost, and
      // re-send it to browsers as they connect.
      if (lastError) {
        surfaceError(lastError);
      }
      devServer.ws.on("connection", () => {
        if (lastError) {
          surfaceError(lastError);
        }
      });
      const base = projectBase();
      devServer.watcher.add(base);
      devServer.watcher.on("all", (_event, file) => {
        const relativePath = projectRelative(base, resolve(file));
        if (relativePath.startsWith("..") || isExcluded(relativePath)) {
          return;
        }
        // handleHotUpdate may have queued this same event already (it
        // runs first when Vite's listener precedes ours); a path still
        // pending needs no second delivery. A path only in the ACTIVE
        // batch is different: this is a fresh fs event, and the
        // in-flight compile may have read the older content.
        if (!pendingPaths.has(relativePath)) {
          void enqueue([relativePath]);
        }
      });
    },

    async buildStart() {
      const outcome = await ensureInitial();
      if (outcome.kind === "error" && config?.command === "build") {
        throw outcome.error;
      }
    },

    async transform(code, id) {
      const path = normalizeId(id);
      if (!path) {
        return null;
      }
      await ensureInitial();
      const current = state;
      if (!current) {
        // No compile+render has succeeded yet; splicing against unknown
        // state is worse than leaving the module alone — the compile
        // error is surfaced already, and the runtime dsql() throw names
        // any file that slips through.
        return null;
      }
      let callsite = current.callsitesByPath.get(path);
      let modules = current.modulesById;
      if (!callsite) {
        return null;
      }

      if (!hashMatches(code, callsite)) {
        // The buffer differs from the compiled file: one filesChanged
        // round-trip, one retry, then a deterministic stale-buffer
        // failure (docs/spec/build-daemon.md, Callsites and freshness).
        const outcome = await enqueue([path]);
        if (outcome.kind === "error") {
          // Absence of a callsite is only meaningful when a successful
          // compile established it — a failed refresh must not let the
          // raw dsql() expression ship.
          throw outcome.error;
        }
        const fresh = state;
        if (!fresh) {
          throw new DsqlRewriteError(
            `${path}: the dsql session was invalidated during the freshness refresh`,
          );
        }
        callsite = fresh.callsitesByPath.get(path);
        modules = fresh.modulesById;
        if (!callsite) {
          return null; // The callsites provably left the file.
        }
        if (!hashMatches(code, callsite)) {
          throw new DsqlRewriteError(
            `${path}: buffer does not match the saved file after a refresh — ` +
              "an upstream transform altered the source before dsql " +
              "(the dsql plugin must run first), or unsaved state was compiled",
          );
        }
      }

      return {
        code: rewriteCallsites({ code, path, callsite, modules, specifierFor }),
        map: null,
      };
    },

    async handleHotUpdate(context) {
      const base = projectBase();
      const relativePath = projectRelative(base, context.file);
      if (relativePath.startsWith("..")) {
        return undefined;
      }
      if (isExcluded(relativePath)) {
        // Renderer/generator output: never HMR-propagate what generation
        // wrote, or generation retriggers itself.
        return [];
      }
      // The watcher handler owns delivery; this hook joins the batch
      // carrying its file instead of delivering the change a second time.
      const outcome = await outcomeFor(relativePath);
      if (outcome.kind === "unchanged") {
        return undefined; // Irrelevant to dsql: normal HMR proceeds.
      }
      // Changed or failed: the full-reload (on success) or the error
      // overlay (on failure) supersede module-level HMR.
      return [];
    },

    async closeBundle() {
      if (config?.command === "build") {
        await client?.shutdown();
        client = null;
      }
    },
  };
}

function hashMatches(code: string, callsite: DsqlCallsite): boolean {
  return sha256Hex(Buffer.from(code, "utf8")) === callsite.contentHash.value;
}

function renderDaemonError(error: DsqlDaemonError, projectBase: string): string {
  const lines = [error.message];
  for (const diagnostic of error.diagnostics) {
    lines.push(renderDiagnostic(diagnostic, projectBase));
  }
  return lines.join("\n");
}

function renderDiagnostic(diagnostic: DsqlDiagnostic, projectBase?: string): string {
  const file = projectBase ? resolve(projectBase, diagnostic.file) : diagnostic.file;
  return (
    `${file}:${diagnostic.range.start}..${diagnostic.range.end} ` +
    `${diagnostic.severity.toLowerCase()} ${diagnostic.source} ${diagnostic.code}: ` +
    diagnostic.message
  );
}

export type { DsqlRenderer, DsqlRenderMap } from "./node.ts";
export { DsqlDaemonError } from "./daemon.ts";
export { DsqlRewriteError } from "./rewrite.ts";
