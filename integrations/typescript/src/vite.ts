import { resolve } from "node:path";
import { buildArtifactsFromGenerated } from "./node.js";
import type { DsqlGenerator } from "./node.js";
import {
  startDsqlDaemon,
  type DsqlDaemon,
  type DsqlDaemonOptions,
} from "./daemon.js";

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

  const compileAndGenerate = async (): Promise<void> => {
    if (!generator) {
      return;
    }
    daemon = daemon ?? startDsqlDaemon(options.daemon);
    const root = options.root ?? config?.root ?? process.cwd();
    const generated = await daemon.compileProject(root);
    const outDir = resolve(root, options.outDir ?? generated.out_dir ?? DEFAULT_OUT_DIR);
    const artifacts = buildArtifactsFromGenerated(generated);
    await generator({
      artifacts,
      root,
      outDir,
      mode: config?.mode ?? "development",
      command: config?.command ?? "serve",
    });
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
        void daemon?.close();
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

      return transformDsqlTags(code, generatedModule);
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
      await daemon?.close();
    },
  };

  return plugin;
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

    const operationName = operationNameFromDsql(body);
    if (!operationName) {
      return match;
    }
    changed = true;

    imports.push(
      `import { ${operationName}Operation as ${localName} } from ${JSON.stringify(
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

function operationNameFromDsql(source: string): string | undefined {
  return /\bquery\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source)?.[1];
}
