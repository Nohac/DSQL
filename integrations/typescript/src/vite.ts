import { relative, resolve } from "node:path";
import { buildArtifactsFromGenerated } from "./node.ts";
import type { DsqlGenerator } from "./node.ts";
import type { DsqlRenderResult } from "./render/types.ts";
import {
  startDsqlDaemon,
  type DsqlDaemon,
  type DsqlDaemonOptions,
} from "./daemon.ts";

export type DsqlVitePluginOptions = {
  readonly generatedModule?: string;
  readonly generator?: DsqlGenerator;
  readonly outDir?: string;
  readonly root?: string;
  readonly daemon?: DsqlDaemonOptions;
  readonly fullReload?: boolean;
};

export type VitePlugin = {
  readonly name: string;
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
};

export type TransformResult =
  | string
  | {
      readonly code: string;
      readonly map?: null;
    }
  | null;

const DEFAULT_GENERATED_MODULE = "/src/generated/dsql/queries";
const DEFAULT_OUT_DIR = "src/generated/dsql";
const SUPPORTED_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx"];
const DSQL_RELEVANT_EXTENSIONS = [".dsql", ".ts", ".tsx", ".js", ".jsx", ".yaml", ".yml"];
const DSQL_TAG_PATTERN =
  /(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*dsql`([\s\S]*?)`\s*;?/g;
const DSQL_CALL_PATTERN =
  /(export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*dsql\s*\(\s*`([\s\S]*?)`\s*\)\s*;?/g;

export function dsql(generator?: DsqlGenerator): VitePlugin;
export function dsql(options?: DsqlVitePluginOptions): VitePlugin;
export function dsql(
  input: DsqlGenerator | DsqlVitePluginOptions = {},
): VitePlugin {
  const options: DsqlVitePluginOptions =
    typeof input === "function" ? { generator: input } : input;
  const generatedModule = options.generatedModule ?? DEFAULT_GENERATED_MODULE;
  const generator = options.generator;
  let daemon: DsqlDaemon | undefined;
  let config: ViteResolvedConfig | undefined;
  let compilePromise: Promise<void> | undefined;
  let sourceFileScopes: readonly { readonly file: string; readonly scope: string }[] = [];
  let renderResults: readonly DsqlRenderResult[] = [];

  const closeDaemon = async (): Promise<void> => {
    const current = daemon;
    daemon = undefined;
    await current?.close();
  };

  const compileAndGenerate = async (): Promise<void> => {
    if (!generator) {
      return;
    }
    daemon = daemon ?? startDsqlDaemon(options.daemon);
    const root = options.root ?? config?.root ?? process.cwd();
    const generated = await daemon.compileProject(root);
    const artifacts = buildArtifactsFromGenerated(generated);
    sourceFileScopes = artifacts.sourceFileScopes;
    const result = await generator({
      artifacts,
      root,
      mode: config?.mode ?? "development",
      command: config?.command ?? "serve",
    });
    renderResults = normalizeRenderResults(result);
  };

  const scheduleCompile = (): Promise<void> => {
    compilePromise = compilePromise ?? compileAndGenerate().finally(() => {
      compilePromise = undefined;
    });
    return compilePromise;
  };

  const plugin: VitePlugin = {
    name: "dsql",
    configResolved(resolved) {
      config = resolved;
    },
    configureServer(server) {
      server.httpServer?.once("close", () => {
        void closeDaemon();
      });
    },
    async buildStart() {
      if (generator) {
        await scheduleCompile();
      }
    },
    transform(code, id) {
      if (!isSupportedFile(id) || !code.includes("dsql")) {
        return null;
      }

      return transformDsqlTags(code, generatedModuleForFile(id));
    },
    async handleHotUpdate(context) {
      const root = options.root ?? config?.root ?? process.cwd();
      const outDir = resolve(root, options.outDir ?? DEFAULT_OUT_DIR);
      if (!isDsqlRelevantFile(context.file, outDir)) {
        return;
      }
      if (!generator) {
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

  function generatedModuleForFile(id: string): string {
    if (renderResults.length === 0) {
      const scope = sourceScopeForFile(id);
      if (scope && isMultiScope()) {
        throw new Error(
          `missing DSQL render metadata for resolution scope ${JSON.stringify(scope)} while transforming ${id}`,
        );
      }
      return generatedModule;
    }
    if (renderResults.length === 1 && !isMultiScope()) {
      const queries = renderResults[0]?.modules.queries;
      return queries ? viteModuleSpecifier(queries) : generatedModule;
    }

    const scope = sourceScopeForFile(id);
    if (!scope) {
      return generatedModule;
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
  generatedModule = DEFAULT_GENERATED_MODULE,
): TransformResult {
  const imports: string[] = [];
  const exports: string[] = [];
  let changed = false;

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

    imports.push(
      `import { ${definition.exportName} as ${localName} } from ${JSON.stringify(
        generatedModule,
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

function isDsqlRelevantFile(file: string, outDir: string): boolean {
  const absoluteFile = resolve(file);
  if (absoluteFile === outDir || absoluteFile.startsWith(`${outDir}/`)) {
    return false;
  }
  if (file.endsWith("dsql.toml")) {
    return true;
  }
  if (file.split(/[\\/]/).includes("schema")) {
    return DSQL_RELEVANT_EXTENSIONS.some((extension) => file.endsWith(extension));
  }
  return DSQL_RELEVANT_EXTENSIONS.some((extension) => file.endsWith(extension));
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
