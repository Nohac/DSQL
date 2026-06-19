import { dirname, isAbsolute, relative, resolve } from "node:path";
import { buildArtifactsFromGenerated } from "./node.ts";
import type { DsqlGenerator } from "./node.ts";
import type { DsqlRenderResult } from "./render/types.ts";
import {
  startDsqlDaemon,
  type DsqlDaemon,
  type DsqlDaemonOptions,
} from "./daemon.ts";

export type DsqlVitePluginOptions = {
  readonly generator: DsqlGenerator;
  readonly root?: string;
  readonly daemon?: DsqlDaemonOptions;
  readonly fullReload?: boolean;
};

type SourceFileScope = {
  readonly file: string;
  readonly scope: string;
};

export type ViteWatchIgnored =
  | string
  | RegExp
  | ((path: string) => boolean)
  | readonly ViteWatchIgnored[];

export type VitePlugin = {
  readonly name: string;
  config?(
    config: ViteUserConfig,
    env: ViteConfigEnv,
  ): ViteUserConfig | void | Promise<ViteUserConfig | void>;
  configResolved?(config: ViteResolvedConfig): void | Promise<void>;
  configureServer?(server: ViteDevServer): void | Promise<void>;
  buildStart?(): void | Promise<void>;
  transform(code: string, id: string): TransformResult;
  handleHotUpdate?(context: ViteHotUpdateContext): void | [] | Promise<void | []>;
  closeBundle?(): void | Promise<void>;
};

export type ViteResolvedConfig = {
  readonly root: string;
  readonly mode: string;
  readonly command: "serve" | "build";
};

export type ViteConfigEnv = {
  readonly mode: string;
  readonly command: "serve" | "build";
};

export type ViteUserConfig = {
  readonly root?: string;
  readonly server?: {
    readonly watch?: {
      readonly ignored?: ViteWatchIgnored;
    };
  };
};

export type ViteHotUpdateContext = {
  readonly file: string;
  readonly server: {
    readonly ws: {
      send(payload: { readonly type: "full-reload" }): void;
    };
  };
};

export type ViteDevServer = {
  readonly httpServer?: {
    once(event: "close", listener: () => void): void;
  };
  readonly watcher?: {
    unwatch(paths: string | readonly string[]): void;
  };
};

export type TransformResult =
  | string
  | {
      readonly code: string;
      readonly map?: null;
    }
  | null;

const SUPPORTED_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx"];
const SCHEMA_RELEVANT_EXTENSIONS = [".yaml", ".yml"];
const DSQL_TAG_PATTERN =
  /(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*dsql`([\s\S]*?)`\s*;?/g;
const DSQL_CALL_PATTERN =
  /(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*dsql\s*\(\s*`([\s\S]*?)`\s*\)\s*;?/g;

export function dsql(generator: DsqlGenerator): VitePlugin;
export function dsql(options: DsqlVitePluginOptions): VitePlugin;
export function dsql(
  input: DsqlGenerator | DsqlVitePluginOptions,
): VitePlugin {
  const options: DsqlVitePluginOptions =
    typeof input === "function" ? { generator: input } : input;
  const generator = options.generator;
  let daemon: DsqlDaemon | undefined;
  let config: ViteResolvedConfig | undefined;
  let configEnv: ViteConfigEnv | undefined;
  let configRoot: string | undefined;
  let compilePromise: Promise<void> | undefined;
  let devServer: ViteDevServer | undefined;
  let sourceFileScopes: readonly SourceFileScope[] = [];
  let renderResults: readonly DsqlRenderResult[] = [];
  const ignoredOutputDirectories = new Set<string>();
  let compiledOnce = false;

  const closeDaemon = async (): Promise<void> => {
    const current = daemon;
    daemon = undefined;
    await current?.close();
  };

  const compileAndGenerate = async (): Promise<void> => {
    daemon = daemon ?? startDsqlDaemon(options.daemon);
    const root = options.root ?? config?.root ?? configRoot ?? process.cwd();
    const generated = await daemon.compileProject(root);
    const artifacts = buildArtifactsFromGenerated(generated);
    sourceFileScopes = artifacts.sourceFileScopes;
    ignoreRenderedOutputDirectories();
    const result = await generator({
      artifacts,
      root,
      mode: config?.mode ?? configEnv?.mode ?? "development",
      command: config?.command ?? configEnv?.command ?? "serve",
    });
    renderResults = normalizeRenderResults(result);
    ignoreRenderedOutputDirectories();
    compiledOnce = true;
  };

  const scheduleCompile = (): Promise<void> => {
    compilePromise = compilePromise ?? compileAndGenerate().finally(() => {
      compilePromise = undefined;
    });
    return compilePromise;
  };

  const plugin: VitePlugin = {
    name: "dsql",
    async config(userConfig, env) {
      configEnv = env;
      configRoot = userConfig.root ? resolve(userConfig.root) : undefined;
      await scheduleCompile();
      const ignored = renderedOutputWatchIgnored(
        userConfig.server?.watch?.ignored,
        renderResults,
      );
      return ignored
        ? {
            server: {
              watch: {
                ignored,
              },
            },
          }
        : undefined;
    },
    configResolved(resolved) {
      config = resolved;
    },
    configureServer(server) {
      devServer = server;
      server.httpServer?.once("close", () => {
        void closeDaemon();
      });
    },
    async buildStart() {
      if (!compiledOnce) {
        await scheduleCompile();
      }
    },
    transform(code, id) {
      if (!isSupportedFile(id) || !code.includes("dsql")) {
        return null;
      }

      return transformDsqlTags(code, () => queryModuleForFile(id));
    },
    async handleHotUpdate(context) {
      if (isRenderedDsqlOutputFile(context.file, renderResults)) {
        return [];
      }
      if (!isDsqlRelevantFile(context.file, renderResults, sourceFileScopes)) {
        return;
      }
      await scheduleCompile();
      if (options.fullReload ?? true) {
        context.server.ws.send({ type: "full-reload" });
        return [];
      }
    },
    async closeBundle() {
      await closeDaemon();
    },
  };

  return plugin;

  function ignoreRenderedOutputDirectories(): void {
    const directories = renderedOutputDirectories(renderResults).filter(
      (directory) => !ignoredOutputDirectories.has(directory),
    );
    if (directories.length === 0) {
      return;
    }
    for (const directory of directories) {
      ignoredOutputDirectories.add(directory);
    }
    ignoreDsqlOutputDirectories(devServer, directories);
  }

  function queryModuleForFile(id: string): string {
    if (renderResults.length === 0) {
      throw new Error(
        `missing DSQL render metadata while transforming ${id}; return renderDsql(...) metadata from the Vite generator`,
      );
    }
    if (renderResults.length === 1 && !isMultiScope()) {
      const queries = renderResults[0]?.modules.queries;
      if (!queries) {
        throw new Error(
          `missing DSQL query module render metadata while transforming ${id}`,
        );
      }
      return viteModuleSpecifier(queries);
    }

    const scope = sourceScopeForFile(id);
    if (!scope) {
      throw new Error(
        `missing DSQL source scope metadata while transforming ${id}`,
      );
    }
    const rendered = renderResults.find((result) => result.scope?.name === scope);
    if (!rendered) {
      throw new Error(
        `missing DSQL render metadata for resolution scope ${JSON.stringify(scope)} while transforming ${id}`,
      );
    }
    return viteModuleSpecifier(rendered.modules.queries);
  }

  function isMultiScope(): boolean {
    return new Set(sourceFileScopes.map((entry) => entry.scope)).size > 1;
  }

  function sourceScopeForFile(id: string): string | undefined {
    const file = resolve(id.split("?")[0] ?? id);
    return sourceFileScopes.find((entry) => resolve(entry.file) === file)?.scope;
  }

  function viteModuleSpecifier(modulePath: string): string {
    const root = options.root ?? config?.root ?? process.cwd();
    if (modulePath.startsWith(".")) {
      return rootAbsoluteSpecifier(root, resolve(root, modulePath));
    }
    if (modulePath.startsWith(root)) {
      return rootAbsoluteSpecifier(root, modulePath);
    }
    return modulePath;
  }
}

function normalizeRenderResults(
  result: Awaited<ReturnType<DsqlGenerator>>,
): readonly DsqlRenderResult[] {
  if (!result) {
    return [];
  }
  return Array.isArray(result) ? result : [result];
}

function rootAbsoluteSpecifier(root: string, absolutePath: string): string {
  return `/${relative(root, absolutePath).split("\\").join("/")}`;
}

export function transformDsqlTags(
  code: string,
  queryModule: string | (() => string),
): TransformResult {
  const imports: string[] = [];
  const exports: string[] = [];
  let changed = false;
  let resolvedQueryModule: string | undefined;

  const replaceDsqlBinding = (
    match: string,
    exportKeyword: string | undefined,
    localName: string,
    body: string,
  ): string => {
    if (body.includes("${")) {
      throw new Error("dsql templates do not support JavaScript interpolation");
    }

    const definition = definitionFromDsql(body);
    if (!definition) {
      return match;
    }
    changed = true;
    resolvedQueryModule ??=
      typeof queryModule === "function" ? queryModule() : queryModule;

    imports.push(
      `import { ${definition.exportName} as ${localName} } from ${JSON.stringify(
        resolvedQueryModule,
      )};`,
    );
    if (exportKeyword) {
      exports.push(`export { ${localName} };`);
    }

    return "";
  };

  const transformed = code
    .replace(DSQL_CALL_PATTERN, replaceDsqlBinding)
    .replace(DSQL_TAG_PATTERN, replaceDsqlBinding);

  if (!changed) {
    return null;
  }

  return {
    code: [...imports, transformed.trimStart(), ...exports]
      .filter((part) => part.length > 0)
      .join("\n"),
    map: null,
  };
}

function isSupportedFile(id: string): boolean {
  const path = id.split("?")[0] ?? id;
  return SUPPORTED_EXTENSIONS.some((extension) => path.endsWith(extension));
}

export function isDsqlRelevantFile(
  file: string,
  renderResults: readonly DsqlRenderResult[],
  sourceFileScopes: readonly SourceFileScope[] = [],
): boolean {
  const absoluteFile = resolve(file);
  if (isRenderedDsqlOutputFile(absoluteFile, renderResults)) {
    return false;
  }
  if (file.endsWith("dsql.toml")) {
    return true;
  }
  if (file.split(/[\\/]/).includes("schema")) {
    return SCHEMA_RELEVANT_EXTENSIONS.some((extension) => file.endsWith(extension));
  }
  if (absoluteFile.endsWith(".dsql")) {
    return true;
  }
  return sourceFileScopes.some((entry) => resolve(entry.file) === absoluteFile);
}

export function isRenderedDsqlOutputFile(
  file: string,
  renderResults: readonly DsqlRenderResult[],
): boolean {
  const absoluteFile = resolve(file);
  return renderedOutputDirectories(renderResults).some((directory) =>
    pathContains(directory, absoluteFile),
  );
}

export function renderedOutputDirectories(
  renderResults: readonly DsqlRenderResult[],
): readonly string[] {
  const directories = new Set<string>();
  for (const result of renderResults) {
    for (const file of result.files) {
      directories.add(dirname(resolve(file.path)));
    }
  }
  return [...directories].sort();
}

export function ignoreDsqlRenderedOutput(
  server: ViteDevServer | undefined,
  renderResults: readonly DsqlRenderResult[],
): void {
  ignoreDsqlOutputDirectories(server, renderedOutputDirectories(renderResults));
}

export function renderedOutputWatchIgnored(
  existing: ViteWatchIgnored | undefined,
  renderResults: readonly DsqlRenderResult[],
): ViteWatchIgnored | undefined {
  const directories = renderedOutputDirectories(renderResults);
  return directories.length > 0
    ? mergeWatchIgnored(existing, directories)
    : undefined;
}

function ignoreDsqlOutputDirectories(
  server: ViteDevServer | undefined,
  directories: readonly string[],
): void {
  if (directories.length === 0 || !server?.watcher) {
    return;
  }

  server.watcher.unwatch(directories);
}

function mergeWatchIgnored(
  existing: ViteWatchIgnored | undefined,
  directories: readonly string[],
): ViteWatchIgnored {
  const directoryMatcher = (path: string): boolean => {
    const absolutePath = resolve(path);
    return directories.some((directory) => pathContains(directory, absolutePath));
  };
  return existing ? [existing, directoryMatcher] : directoryMatcher;
}

function pathContains(directory: string, file: string): boolean {
  const relativePath = relative(resolve(directory), resolve(file));
  return (
    relativePath === "" ||
    (relativePath.length > 0 &&
      !relativePath.startsWith("..") &&
      !isAbsolute(relativePath))
  );
}

function definitionFromDsql(
  source: string,
): { readonly exportName: string } | undefined {
  const operationName = /\bquery\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source)?.[1];
  if (operationName) {
    return { exportName: `${operationName}Operation` };
  }
  const fragmentName = /\bfragment\s+([A-Za-z_][A-Za-z0-9_]*)\s+on\b/.exec(
    source,
  )?.[1];
  if (fragmentName) {
    return { exportName: `${fragmentName}Fragment` };
  }
  return undefined;
}
